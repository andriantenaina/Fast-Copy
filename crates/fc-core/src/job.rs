//! Job execution: conflict handling, the small-file worker pool and the large-file
//! lane, durability batches, move semantics, resume and the final report.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::engine::{self, Ctx};
use crate::error::{Error, IoContext, Result};
use crate::journal::{journal_path, Journal, Record, ResumeState};
use crate::names::numbered;
use crate::plan::{Item, ItemKind, Plan};
use crate::split::{SplitManifest, SPLIT_CHUNK};
use crate::sys;
use crate::types::*;

/// Options for [`execute`].
pub struct ExecOptions {
    /// Write a crash-safe journal enabling resume (default true).
    pub journal: bool,
    /// Directory for the journal (default: [`crate::journal::jobs_dir`]).
    pub journal_dir: Option<PathBuf>,
    /// Run even if the plan has blocking issues.
    pub force: bool,
    /// State recovered from an interrupted run.
    pub resume: Option<ResumeState>,
    /// Shared counters (lets a UI read progress directly).
    pub stats: Option<Arc<Stats>>,
    /// Emit one event per file (GUI lists); off for huge jobs in the CLI.
    pub file_events: bool,
    pub progress_interval: Duration,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions {
            journal: true,
            journal_dir: None,
            force: false,
            resume: None,
            stats: None,
            file_events: false,
            progress_interval: Duration::from_millis(100),
        }
    }
}

enum Decision {
    Proceed { replace: bool, path: PathBuf },
    Skip,
    AlreadyDone,
}

enum Outcome {
    /// Committed; `handle` still needs the batch device flush (unbuffered small file).
    Done { bytes: u64, handle: Option<File>, delete: Vec<PathBuf> },
    /// Written through the cache; verified and committed when the batch is flushed.
    Deferred(Box<Deferred>),
    Skipped(String),
}

struct Deferred {
    i: usize,
    w: engine::Written,
    final_path: PathBuf,
    replace: bool,
}

#[derive(Default)]
struct Batch {
    idxs: Vec<usize>,
    deletes: Vec<PathBuf>,
    last: Option<File>,
    deferred: Vec<Deferred>,
}

struct Run<'a> {
    plan: &'a Plan,
    ctx: Ctx<'a>,
    sink: &'a dyn EventSink,
    ui: &'a dyn Interaction,
    journal: Option<Journal>,
    resume: ResumeState,
    resuming: bool,
    apply_all: Mutex<Option<OnConflict>>,
    errors: Mutex<Vec<FileError>>,
    locked: Mutex<Vec<(usize, PathBuf, PathBuf)>>,
    files_copied: AtomicU64,
    bytes_copied: AtomicU64,
    file_events: bool,
    split_chunk: u64,
}

/// Execute a plan.
pub fn execute(plan: &Plan, control: &Control, sink: &dyn EventSink, ui: &dyn Interaction, opts: ExecOptions) -> Result<Report> {
    let blocking = plan.blocking_issues();
    if !opts.force && !blocking.is_empty() {
        return Err(Error::Blocked(blocking.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join(" ; ")));
    }
    let stats = opts.stats.clone().unwrap_or_default();
    let warnings = Mutex::new(Vec::new());
    let resuming = opts.resume.is_some();
    let resume = opts.resume.unwrap_or_default();

    let journal = if opts.journal {
        let path = opts.journal_dir.as_ref().map(|d| d.join(format!("{}.jsonl", plan.id))).unwrap_or_else(|| journal_path(&plan.id));
        Some(if resuming && path.exists() {
            Journal::reopen(&path)?
        } else {
            match &opts.journal_dir {
                Some(d) => Journal::create_in(d, plan)?,
                None => Journal::create(plan)?,
            }
        })
    } else {
        None
    };

    // Moves must never delete a source that was not verified on the destination.
    let verify = if plan.spec.mode == Mode::Move && plan.spec.verify == VerifyMode::Fast { VerifyMode::Full } else { plan.spec.verify };
    let split_chunk = plan.spec.split_chunk_override.filter(|c| *c >= 4096 && c % 4096 == 0).unwrap_or(SPLIT_CHUNK);

    let run = Run {
        plan,
        ctx: Ctx { profile: &plan.profile, control, stats: &stats, verify, dst_fs: &plan.dst_volume.fs, warnings: &warnings },
        sink,
        ui,
        journal,
        resume,
        resuming,
        apply_all: Mutex::new(None),
        errors: Mutex::new(Vec::new()),
        locked: Mutex::new(Vec::new()),
        files_copied: AtomicU64::new(0),
        bytes_copied: AtomicU64::new(0),
        file_events: opts.file_events,
        split_chunk,
    };

    stats.bytes_total.store(plan.total_bytes, Ordering::Relaxed);
    stats.files_total.store(plan.total_files, Ordering::Relaxed);
    sink.event(Event::Planned { files: plan.total_files, bytes: plan.total_bytes, dirs: plan.total_dirs });
    stats.set_phase(Phase::Copying);

    let finished = AtomicBool::new(false);
    let start = Instant::now();
    let result = std::thread::scope(|s| {
        s.spawn(|| progress_loop(&stats, control, sink, &finished, opts.progress_interval));
        let r = run.run_all();
        finished.store(true, Ordering::SeqCst);
        r
    });

    if result.is_ok() && plan.spec.use_vss {
        run.copy_locked_via_vss();
    }
    let cancelled = matches!(result, Err(Error::Cancelled));
    if let Err(e) = &result {
        if !cancelled {
            run.errors.lock().unwrap().push(FileError { path: String::new(), error: e.to_string() });
        }
    }
    stats.set_phase(Phase::Done);
    let errors = std::mem::take(&mut *run.errors.lock().unwrap());
    let status = if cancelled {
        JobStatus::Cancelled
    } else if result.is_err() {
        JobStatus::Failed
    } else if errors.is_empty() {
        JobStatus::Completed
    } else {
        JobStatus::CompletedWithErrors
    };
    let elapsed = start.elapsed();
    let bytes = run.bytes_copied.load(Ordering::Relaxed);
    let report = Report {
        job_id: plan.id.clone(),
        status,
        files_copied: run.files_copied.load(Ordering::Relaxed),
        files_skipped: stats.files_skipped.load(Ordering::Relaxed),
        files_failed: stats.files_failed.load(Ordering::Relaxed),
        bytes_copied: bytes,
        elapsed_ms: elapsed.as_millis() as u64,
        avg_speed: if elapsed.as_secs_f64() > 0.0 { bytes as f64 / elapsed.as_secs_f64() } else { 0.0 },
        errors,
        renamed: plan.renamed.clone(),
        warnings: std::mem::take(&mut *warnings.lock().unwrap()),
        locked: std::mem::take(&mut *run.locked.lock().unwrap()).into_iter().map(|(_, s, _)| s).collect(),
    };

    if let Some(j) = &run.journal {
        let st = match status {
            JobStatus::Completed => "completed",
            JobStatus::CompletedWithErrors => "completed-with-errors",
            JobStatus::Cancelled => "cancelled",
            JobStatus::Failed => "failed",
        };
        let _ = j.append(&Record::End { status: st.into() });
        let _ = j.sync();
        if status == JobStatus::Completed {
            let _ = std::fs::remove_file(j.path());
        }
    }
    sink.event(Event::Finished(report.clone()));
    Ok(report)
}

/// Resume an interrupted job from its journal (`id` or path to the `.jsonl`).
pub fn resume(id_or_path: &str, control: &Control, sink: &dyn EventSink, ui: &dyn Interaction, mut opts: ExecOptions) -> Result<Report> {
    let path = if Path::new(id_or_path).exists() { PathBuf::from(id_or_path) } else { journal_path(id_or_path) };
    let (plan, state) = Journal::load(&path)?;
    opts.resume = Some(state);
    opts.force = true; // the plan was accepted when the job started
    if opts.journal_dir.is_none() {
        opts.journal_dir = path.parent().map(|p| p.to_path_buf());
    }
    execute(&plan, control, sink, ui, opts)
}

fn progress_loop(stats: &Stats, control: &Control, sink: &dyn EventSink, finished: &AtomicBool, every: Duration) {
    let mut last_bytes = stats.bytes_done.load(Ordering::Relaxed);
    let mut last_t = Instant::now();
    let mut speed = 0f64;
    loop {
        let done = finished.load(Ordering::SeqCst);
        std::thread::sleep(if done { Duration::ZERO } else { every });
        let now = Instant::now();
        let bytes = stats.bytes_done.load(Ordering::Relaxed);
        let dt = now.duration_since(last_t).as_secs_f64();
        if dt > 0.0 && !control.is_paused() {
            let inst = bytes.saturating_sub(last_bytes) as f64 / dt;
            speed = if speed == 0.0 { inst } else { speed * 0.8 + inst * 0.2 };
        }
        last_bytes = bytes;
        last_t = now;
        let total = stats.bytes_total.load(Ordering::Relaxed);
        let done_b = bytes.min(total);
        let snap = Snapshot {
            phase: stats.phase(),
            bytes_total: total,
            bytes_done: done_b,
            bytes_verified: stats.bytes_verified.load(Ordering::Relaxed).min(total),
            files_total: stats.files_total.load(Ordering::Relaxed),
            files_done: stats.files_done.load(Ordering::Relaxed),
            files_skipped: stats.files_skipped.load(Ordering::Relaxed),
            files_failed: stats.files_failed.load(Ordering::Relaxed),
            current: stats.current.lock().unwrap().clone(),
            speed,
            elapsed_ms: stats.start.elapsed().as_millis() as u64,
            eta_secs: if speed > 1.0 { Some(((total - done_b) as f64 / speed) as u64) } else { None },
            paused: control.is_paused(),
        };
        sink.event(Event::Progress(snap));
        if done {
            break;
        }
    }
}

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    crate::plan::mtime_ms(meta)
}

/// Copy a single file with the full safety pipeline (temp file, flush, verify, atomic
/// rename). Used by the elevated VSS helper.
pub fn copy_one(src: &Path, dst: &Path, verify: VerifyMode) -> Result<u64> {
    let dst_vol = crate::volume::detect(dst);
    let profile = crate::plan::Profile::choose(&crate::volume::detect(src), &dst_vol, None);
    let control = Control::new();
    let stats = Stats::default();
    let warnings = Mutex::new(Vec::new());
    let ctx = Ctx { profile: &profile, control: &control, stats: &stats, verify, dst_fs: &dst_vol.fs, warnings: &warnings };
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(sys::long_path(p)).ctx("création du dossier", p)?;
    }
    let tmp = crate::util::temp_path(dst);
    let r = (|| -> Result<u64> {
        let w = engine::write_file(&ctx, src, dst, None, std::fs::metadata(sys::long_path(src)).map(|m| m.len()).unwrap_or(0), false, &mut |_| Ok(()))?;
        sys::full_sync(&w.file).ctx("vidage", &w.tmp)?;
        engine::verify_written(&ctx, &w, src)?;
        engine::apply_metadata(&ctx, &w.file, &w.tmp, src, &w.meta, &w.streams);
        engine::commit(&w.tmp, dst, true)?;
        engine::apply_final_attributes(dst, &w.meta, w.hidden);
        Ok(w.size)
    })();
    if r.is_err() {
        engine::discard_tmp(&tmp);
    }
    r
}

impl Run<'_> {
    /// Retry files that were locked, through an elevated VSS snapshot copy.
    fn copy_locked_via_vss(&self) {
        let pending = std::mem::take(&mut *self.locked.lock().unwrap());
        if pending.is_empty() {
            return;
        }
        let pairs: Vec<crate::vss::VssPair> = pending.iter().map(|(_, s, d)| crate::vss::VssPair { src: s.clone(), dst: d.clone() }).collect();
        match crate::vss::copy_locked(&pairs, self.ctx.verify) {
            Ok(results) => {
                let mut still = Vec::new();
                for ((i, s, d), r) in pending.into_iter().zip(results) {
                    let it = &self.plan.items[i];
                    if r.ok {
                        self.errors.lock().unwrap().retain(|e| e.path != it.rel);
                        self.ctx.stats.files_failed.fetch_sub(1, Ordering::Relaxed);
                        self.ctx.stats.files_done.fetch_add(1, Ordering::Relaxed);
                        self.files_copied.fetch_add(1, Ordering::Relaxed);
                        self.bytes_copied.fetch_add(r.bytes, Ordering::Relaxed);
                        self.journal(Record::Done { i });
                        if self.plan.spec.mode == Mode::Move {
                            self.ctx.warn(format!("« {} » copié via VSS mais non supprimé (fichier en cours d'utilisation)", it.rel));
                        }
                    } else {
                        self.ctx.warn(format!("VSS : « {} » : {}", it.rel, r.error.unwrap_or_default()));
                        still.push((i, s, d));
                    }
                }
                *self.locked.lock().unwrap() = still;
            }
            Err(e) => {
                self.ctx.warn(format!("copie VSS impossible : {e}"));
                *self.locked.lock().unwrap() = pending;
            }
        }
    }

    fn journal(&self, r: Record) {
        if let Some(j) = &self.journal {
            let _ = j.append(&r);
        }
    }

    fn run_all(&self) -> Result<()> {
        let plan = self.plan;
        std::fs::create_dir_all(sys::long_path(&plan.spec.dest)).ctx("création de la destination", &plan.spec.dest)?;

        let mut skip_set = std::collections::HashSet::new();
        if plan.same_volume {
            self.fast_move_trees(&mut skip_set);
        }

        // Directories first, in plan order (parents before children).
        for (i, it) in plan.items.iter().enumerate() {
            if it.kind != ItemKind::Dir || skip_set.contains(&i) {
                continue;
            }
            self.ctx.control.checkpoint()?;
            let p = plan.dst_path(it);
            match std::fs::create_dir_all(sys::long_path(&p)) {
                Ok(()) => {}
                Err(e) => self.fail(i, it, &Error::io("création du dossier", &p, e)),
            }
        }

        let threshold = plan.profile.large_threshold;
        let mut small = Vec::new();
        let mut large = Vec::new();
        for (i, it) in plan.items.iter().enumerate() {
            if skip_set.contains(&i) {
                continue;
            }
            if self.resume.done.contains(&i) || self.resume.skipped.contains(&i) {
                self.account_resumed(it);
                continue;
            }
            match it.kind {
                ItemKind::Dir => {}
                ItemKind::File if it.size < threshold || plan.same_volume => small.push(i),
                ItemKind::Symlink => small.push(i),
                _ => large.push(i),
            }
        }

        if plan.profile.parallel_lanes {
            std::thread::scope(|s| {
                let h = s.spawn(|| self.large_lane(&large));
                let a = self.small_pool(&small);
                let b = h.join().unwrap_or(Err(Error::Other("voie gros fichiers interrompue".into())));
                a.and(b)
            })?;
        } else {
            self.small_pool(&small)?;
            self.large_lane(&large)?;
        }

        self.ctx.stats.set_phase(Phase::Finalizing);
        self.finalize_dirs();
        Ok(())
    }

    fn account_resumed(&self, it: &Item) {
        let st = self.ctx.stats;
        if it.kind != ItemKind::Dir {
            st.add_done(it.size);
            st.add_verified(it.size);
            st.files_done.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Same-volume move: rename whole source trees at once when possible.
    fn fast_move_trees(&self, skip: &mut std::collections::HashSet<usize>) {
        let plan = self.plan;
        for src in &plan.spec.sources {
            let Some(base) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let Some((top_i, top)) = plan.items.iter().enumerate().find(|(_, it)| it.rel == base && it.src == *src) else { continue };
            // Only when no descendant needed renaming.
            let prefix = format!("{base}/");
            if plan.renamed.iter().any(|(from, _)| from == &base || from.starts_with(&prefix)) {
                continue;
            }
            let target = plan.dst_path(top);
            if std::fs::symlink_metadata(sys::long_path(&target)).is_ok() {
                continue;
            }
            if std::fs::rename(sys::long_path(src), sys::long_path(&target)).is_ok() {
                if let Some(dir) = target.parent() {
                    let _ = sys::sync_dir(dir);
                }
                for (i, it) in plan.items.iter().enumerate() {
                    if i == top_i || it.rel.starts_with(&prefix) {
                        skip.insert(i);
                        if it.kind != ItemKind::Dir {
                            self.ctx.stats.add_done(it.size);
                            self.ctx.stats.add_verified(it.size);
                            self.ctx.stats.files_done.fetch_add(1, Ordering::Relaxed);
                            self.files_copied.fetch_add(1, Ordering::Relaxed);
                            self.bytes_copied.fetch_add(it.size, Ordering::Relaxed);
                        }
                        self.journal(Record::Done { i });
                    }
                }
            }
        }
    }

    fn small_pool(&self, idxs: &[usize]) -> Result<()> {
        if idxs.is_empty() {
            return Ok(());
        }
        let next = AtomicUsize::new(0);
        let workers = self.plan.profile.small_workers.clamp(1, idxs.len());
        let stop = AtomicBool::new(false);
        let first_err: Mutex<Option<Error>> = Mutex::new(None);
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| {
                    let mut batch = Batch::default();
                    let mut since_sync = 0usize;
                    let res = (|| -> Result<()> {
                        loop {
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            let k = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&i) = idxs.get(k) else { break };
                            if self.run_item(i, &mut batch, true)? {
                                since_sync += 1;
                            }
                            if since_sync >= self.plan.profile.sync_every {
                                self.flush_batch(&mut batch)?;
                                since_sync = 0;
                            }
                        }
                        self.flush_batch(&mut batch)
                    })();
                    if let Err(e) = res {
                        stop.store(true, Ordering::Relaxed);
                        // Keep what was already committed durable even when stopping.
                        let _ = self.flush_batch(&mut batch);
                        let mut fe = first_err.lock().unwrap();
                        if fe.is_none() {
                            *fe = Some(e);
                        }
                    }
                });
            }
        });
        match first_err.into_inner().unwrap() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn large_lane(&self, idxs: &[usize]) -> Result<()> {
        for &i in idxs {
            let mut batch = Batch::default();
            self.run_item(i, &mut batch, false)?;
            self.flush_batch(&mut batch)?;
        }
        Ok(())
    }

    /// Verify and commit deferred files, make the batch durable, then journal it and
    /// delete move sources.
    fn flush_batch(&self, b: &mut Batch) -> Result<()> {
        let deferred = std::mem::take(&mut b.deferred);
        let mut cancelled = false;
        for d in deferred {
            let it = &self.plan.items[d.i];
            if cancelled {
                engine::discard_tmp(&d.w.tmp);
                continue;
            }
            let ctx = &self.ctx;
            let r = (|| -> Result<()> {
                // Cache-bypassing re-read: forces any remaining cached data to the device.
                engine::verify_written(ctx, &d.w, &it.src)?;
                engine::apply_metadata(ctx, &d.w.file, &d.w.tmp, &it.src, &d.w.meta, &d.w.streams);
                self.journal(Record::Intent { i: d.i });
                engine::commit(&d.w.tmp, &d.final_path, d.replace)?;
                engine::apply_final_attributes(&d.final_path, &d.w.meta, d.w.hidden);
                Ok(())
            })();
            match r {
                Ok(()) => {
                    self.ctx.stats.files_done.fetch_add(1, Ordering::Relaxed);
                    self.files_copied.fetch_add(1, Ordering::Relaxed);
                    self.bytes_copied.fetch_add(d.w.size, Ordering::Relaxed);
                    if self.file_events {
                        self.sink.event(Event::FileDone { path: it.rel.clone(), bytes: d.w.size });
                    }
                    b.idxs.push(d.i);
                    b.deletes.extend(self.deletes_for(it));
                    b.last = Some(d.w.file);
                }
                Err(Error::Cancelled) => {
                    engine::discard_tmp(&d.w.tmp);
                    cancelled = true;
                }
                Err(e) => {
                    // Retry this file on the synchronous path (fresh write, immediate verify).
                    engine::discard_tmp(&d.w.tmp);
                    drop(d.w);
                    self.ctx.warn(format!("{e} — nouvelle copie de « {} »", it.rel));
                    let mut single = Batch::default();
                    match self.run_item(d.i, &mut single, false) {
                        Ok(_) => {
                            b.idxs.append(&mut single.idxs);
                            b.deletes.append(&mut single.deletes);
                            if single.last.is_some() {
                                b.last = single.last;
                            }
                        }
                        Err(Error::Cancelled) => cancelled = true,
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        if let Some(f) = b.last.take() {
            sys::full_sync(&f).map_err(|e| Error::io("vidage", Path::new("<lot>"), e))?;
        }
        for &i in &b.idxs {
            self.journal(Record::Done { i });
        }
        if let Some(j) = &self.journal {
            let _ = j.flush();
        }
        for p in b.deletes.drain(..) {
            let lp = sys::long_path(&p);
            let _ = sys::clear_readonly(&lp);
            if let Err(e) = std::fs::remove_file(&lp) {
                self.ctx.warn(format!("source non supprimée « {} » : {e}", p.display()));
            }
        }
        b.idxs.clear();
        if cancelled {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    /// Process one item with retries. Returns true if something was committed.
    fn run_item(&self, i: usize, batch: &mut Batch, defer: bool) -> Result<bool> {
        let it = &self.plan.items[i];
        self.ctx.control.checkpoint()?;
        self.ctx.stats.set_current(&it.rel);
        let mut attempts = 0;
        loop {
            match self.process(i, it, defer && attempts == 0) {
                Ok(Outcome::Done { bytes, handle, delete }) => {
                    self.ctx.stats.files_done.fetch_add(1, Ordering::Relaxed);
                    self.files_copied.fetch_add(1, Ordering::Relaxed);
                    self.bytes_copied.fetch_add(bytes, Ordering::Relaxed);
                    if self.file_events {
                        self.sink.event(Event::FileDone { path: it.rel.clone(), bytes });
                    }
                    batch.idxs.push(i);
                    batch.deletes.extend(delete);
                    if handle.is_some() {
                        batch.last = handle;
                    }
                    return Ok(true);
                }
                Ok(Outcome::Deferred(d)) => {
                    batch.deferred.push(*d);
                    return Ok(true);
                }
                Ok(Outcome::Skipped(reason)) => {
                    self.ctx.stats.files_skipped.fetch_add(1, Ordering::Relaxed);
                    self.ctx.stats.add_done(it.size);
                    self.ctx.stats.add_verified(it.size);
                    self.journal(Record::Skip { i });
                    if self.file_events {
                        self.sink.event(Event::FileSkipped { path: it.rel.clone(), reason });
                    }
                    return Ok(false);
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e @ (Error::VerifyMismatch(_) | Error::SourceChanged(_))) if attempts < 2 => {
                    attempts += 1;
                    self.ctx.warn(format!("{e} — nouvelle tentative ({attempts}/2)"));
                }
                Err(e) if e.is_disk_full() => {
                    let avail = crate::volume::detect(&self.plan.spec.dest).free_bytes;
                    match self.ui.disk_full(&it.rel, it.size, avail) {
                        DiskFullAction::Retry => continue,
                        DiskFullAction::Skip => {
                            self.fail(i, it, &e);
                            return Ok(false);
                        }
                        DiskFullAction::Abort => {
                            self.fail(i, it, &e);
                            self.ctx.control.cancel();
                            return Err(Error::Cancelled);
                        }
                    }
                }
                Err(e) => {
                    if e.is_locked() {
                        self.locked.lock().unwrap().push((i, it.src.clone(), self.plan.dst_path(it)));
                    }
                    self.fail(i, it, &e);
                    return Ok(false);
                }
            }
        }
    }

    fn fail(&self, i: usize, it: &Item, e: &Error) {
        if it.kind != ItemKind::Dir {
            self.ctx.stats.files_failed.fetch_add(1, Ordering::Relaxed);
        }
        let msg = e.to_string();
        self.journal(Record::Failed { i, error: msg.clone() });
        self.errors.lock().unwrap().push(FileError { path: it.rel.clone(), error: msg.clone() });
        self.sink.event(Event::FileFailed { path: it.rel.clone(), error: msg });
    }

    fn deletes_for(&self, it: &Item) -> Vec<PathBuf> {
        if self.plan.spec.mode != Mode::Move {
            return vec![];
        }
        match it.kind {
            ItemKind::Join => {
                let mut v = it.parts.clone();
                v.push(it.src.clone());
                if let Ok(m) = SplitManifest::load(&it.src) {
                    let dir = it.src.parent().unwrap_or(Path::new("")).to_path_buf();
                    v.extend(m.companion_files().into_iter().map(|c| dir.join(c)).filter(|p| p.exists()));
                }
                v
            }
            _ => vec![it.src.clone()],
        }
    }

    fn process(&self, i: usize, it: &Item, defer: bool) -> Result<Outcome> {
        let plan = self.plan;
        let final0 = plan.dst_path(it);
        // Destination used to detect a conflict (split sets are identified by their manifest).
        let probe = if it.kind == ItemKind::Split {
            final0.with_file_name(SplitManifest::manifest_name(&final0.file_name().unwrap_or_default().to_string_lossy()))
        } else {
            final0.clone()
        };
        let (replace, final_path) = match std::fs::symlink_metadata(sys::long_path(&probe)) {
            Ok(dm) => match self.decide(i, it, &dm, &final0)? {
                Decision::Skip => return Ok(Outcome::Skipped("existe déjà".into())),
                Decision::AlreadyDone => {
                    return Ok(Outcome::Done { bytes: it.size, handle: None, delete: self.deletes_for(it) });
                }
                Decision::Proceed { replace, path } => (replace, path),
            },
            Err(_) => (false, final0),
        };

        match it.kind {
            ItemKind::Dir => Ok(Outcome::Skipped(String::new())),
            ItemKind::Symlink => self.copy_symlink(it, &final_path, replace),
            ItemKind::Split => {
                engine::write_split(&self.ctx, &it.src, &final_path, self.split_chunk)?;
                Ok(Outcome::Done { bytes: it.size, handle: None, delete: self.deletes_for(it) })
            }
            ItemKind::Join => {
                let w = engine::write_join(&self.ctx, &it.src, &it.parts, &final_path)?;
                self.journal(Record::Intent { i });
                engine::commit(&w.tmp, &final_path, replace)?;
                Ok(Outcome::Done { bytes: w.size, handle: None, delete: self.deletes_for(it) })
            }
            ItemKind::File if plan.same_volume => {
                let lf = sys::long_path(&final_path);
                if replace {
                    let _ = sys::clear_readonly(&lf);
                }
                std::fs::rename(sys::long_path(&it.src), &lf).ctx("déplacement", &it.src)?;
                if let Some(d) = final_path.parent() {
                    let _ = sys::sync_dir(d);
                }
                self.ctx.stats.add_done(it.size);
                self.ctx.stats.add_verified(it.size);
                Ok(Outcome::Done { bytes: it.size, handle: None, delete: vec![] })
            }
            ItemKind::File => self.copy_file(i, it, &final_path, replace, defer),
        }
    }

    fn copy_file(&self, i: usize, it: &Item, final_path: &Path, replace: bool, defer: bool) -> Result<Outcome> {
        let ctx = &self.ctx;
        let tmp = crate::util::temp_path(final_path);

        // Copy-on-write clone on the same APFS / ReFS volume: instant and identical.
        if self.plan.spec.mode == Mode::Copy
            && self.plan.dst_volume.fs.supports_clone()
            && self.plan.src_volume.same_volume(&self.plan.dst_volume)
            && it.size > 0
            && engine::try_clone(&it.src, &tmp)
        {
            let meta = std::fs::metadata(sys::long_path(&it.src)).ctx("lecture des métadonnées", &it.src)?;
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(sys::long_path(&tmp)) {
                engine::apply_metadata(ctx, &f, &tmp, &it.src, &meta, &sys::list_streams(&sys::long_path(&it.src)));
            }
            self.journal(Record::Intent { i });
            engine::commit(&tmp, final_path, replace)?;
            engine::apply_final_attributes(final_path, &meta, false);
            ctx.stats.add_done(it.size);
            ctx.stats.add_verified(it.size);
            return Ok(Outcome::Done { bytes: it.size, handle: None, delete: self.deletes_for(it) });
        }

        let resume_off = self.resume.checkpoints.get(&i).copied();
        let journal = &self.journal;
        let mut on_ckpt = |off: u64| -> Result<()> {
            if let Some(j) = journal {
                j.append(&Record::Ckpt { i, off })?;
                j.sync()?;
            }
            Ok(())
        };
        let defer = defer
            && self.plan.profile.small_buffered
            && ctx.verify != VerifyMode::Fast
            && it.size < self.plan.profile.large_threshold;
        let result = (|| -> Result<Outcome> {
            let w = engine::write_file(ctx, &it.src, final_path, resume_off, it.size, defer, &mut on_ckpt)?;
            if defer {
                return Ok(Outcome::Deferred(Box::new(Deferred { i, w, final_path: final_path.to_path_buf(), replace })));
            }
            let large = w.size >= self.plan.profile.large_threshold;
            let flush_now = large || !self.plan.profile.nocache_write;
            if flush_now {
                sys::full_sync(&w.file).ctx("vidage", &w.tmp)?;
            }
            engine::verify_written(ctx, &w, &it.src)?;
            engine::apply_metadata(ctx, &w.file, &w.tmp, &it.src, &w.meta, &w.streams);
            self.journal(Record::Intent { i });
            engine::commit(&w.tmp, final_path, replace)?;
            engine::apply_final_attributes(final_path, &w.meta, w.hidden);
            let handle = if flush_now { None } else { Some(w.file) };
            Ok(Outcome::Done { bytes: w.size, handle, delete: self.deletes_for(it) })
        })();
        if let Err(e) = &result {
            // Keep the partial file only when cancelled (resume continues from the checkpoint).
            if !matches!(e, Error::Cancelled) {
                engine::discard_tmp(&tmp);
            }
        }
        result
    }

    #[cfg(unix)]
    fn copy_symlink(&self, it: &Item, final_path: &Path, replace: bool) -> Result<Outcome> {
        let target = std::fs::read_link(&it.src).ctx("lecture du lien", &it.src)?;
        if replace {
            let _ = std::fs::remove_file(final_path);
        }
        match std::os::unix::fs::symlink(&target, final_path) {
            Ok(()) => Ok(Outcome::Done { bytes: 0, handle: None, delete: self.deletes_for(it) }),
            Err(e) => {
                self.ctx.warn(format!("lien symbolique « {} » non recréé ({}) : {e}", it.rel, self.plan.dst_volume.fs.label()));
                Ok(Outcome::Skipped("lien symbolique non supporté".into()))
            }
        }
    }

    #[cfg(windows)]
    fn copy_symlink(&self, it: &Item, _final_path: &Path, _replace: bool) -> Result<Outcome> {
        self.ctx.warn(format!("lien symbolique / jonction « {} » ignoré", it.rel));
        Ok(Outcome::Skipped("lien symbolique".into()))
    }

    /// Decide what to do with an existing destination.
    fn decide(&self, i: usize, it: &Item, dm: &std::fs::Metadata, final_path: &Path) -> Result<Decision> {
        if it.kind == ItemKind::Dir {
            return Ok(Decision::Proceed { replace: false, path: final_path.to_path_buf() });
        }
        if dm.is_dir() {
            return Err(Error::Other(format!("un dossier « {} » existe déjà à la destination", final_path.display())));
        }
        let mut recorded = self.resume.decisions.get(&i).copied();
        if self.resuming && it.kind == ItemKind::File && dm.len() == it.size {
            // Possibly our own commit from the interrupted run: compare contents.
            let same = engine::fingerprint_file(&self.ctx, &it.src)
                .and_then(|(_, a)| engine::fingerprint_file(&self.ctx, final_path).map(|(_, b)| a == b))
                .unwrap_or(false);
            if same && (self.resume.intents.contains(&i) || recorded.is_some() || self.plan.spec.on_conflict != OnConflict::KeepBoth) {
                self.ctx.stats.add_done(it.size);
                self.ctx.stats.add_verified(it.size);
                return Ok(Decision::AlreadyDone);
            }
        }
        if recorded.is_none() && self.resume.intents.contains(&i) {
            recorded = Some(OnConflict::Overwrite);
        }
        let policy = match recorded {
            Some(p) => p,
            None => {
                let mut all = self.apply_all.lock().unwrap();
                match all.or(Some(self.plan.spec.on_conflict)).unwrap() {
                    OnConflict::Ask => {
                        let info = ConflictInfo {
                            path: it.rel.clone(),
                            src_size: it.size,
                            src_mtime_ms: it.mtime_ms,
                            dst_size: dm.len(),
                            dst_mtime_ms: mtime_of(dm),
                        };
                        // Holding the lock serializes questions between workers.
                        let d = self.ui.resolve_conflict(&info);
                        let action = if d.action == OnConflict::Ask { OnConflict::Skip } else { d.action };
                        if d.apply_to_all {
                            *all = Some(action);
                        }
                        action
                    }
                    p => p,
                }
            }
        };
        self.journal(Record::Decide { i, action: policy });
        let tol = self.plan.dst_volume.fs.mtime_tolerance_ms() as i64;
        let dst_m = mtime_of(dm);
        let decision = match policy {
            OnConflict::Overwrite | OnConflict::Ask => Decision::Proceed { replace: true, path: final_path.to_path_buf() },
            OnConflict::Skip => Decision::Skip,
            OnConflict::Newer => {
                if it.mtime_ms > dst_m + tol {
                    Decision::Proceed { replace: true, path: final_path.to_path_buf() }
                } else {
                    Decision::Skip
                }
            }
            OnConflict::IfDifferent => {
                // FAT stores local time: also accept exact whole-hour offsets (DST / time zone).
                let diff = (it.mtime_ms - dst_m).abs();
                let hour_shift = self.plan.dst_volume.fs.is_fat_family() && diff % 3_600_000 <= tol && diff <= 14 * 3_600_000;
                if dm.len() != it.size || (diff > tol && !hour_shift) {
                    Decision::Proceed { replace: true, path: final_path.to_path_buf() }
                } else {
                    Decision::Skip
                }
            }
            OnConflict::KeepBoth => {
                let name = final_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let mut n = 2;
                loop {
                    let cand = final_path.with_file_name(numbered(&name, n));
                    if std::fs::symlink_metadata(sys::long_path(&cand)).is_err() {
                        break Decision::Proceed { replace: false, path: cand };
                    }
                    n += 1;
                }
            }
        };
        Ok(decision)
    }

    /// Restore directory dates (deepest first); remove emptied source dirs on move.
    fn finalize_dirs(&self) {
        let plan = self.plan;
        for it in plan.items.iter().rev().filter(|it| it.kind == ItemKind::Dir) {
            let p = plan.dst_path(it);
            if it.mtime_ms > 0 {
                let t = SystemTime::UNIX_EPOCH + Duration::from_millis(it.mtime_ms as u64);
                let _ = sys::set_dir_mtime(&sys::long_path(&p), t);
            }
            if plan.spec.mode == Mode::Move && it.src.exists() {
                let _ = std::fs::remove_dir(sys::long_path(&it.src));
            }
        }
    }
}
