//! Shared application state: the job queue, built plans, pending interaction requests
//! and the bridge between fc-core callbacks and Tauri events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fc_core::plan::{Issue, Plan, Severity};
use fc_core::queue::{JobQueue, QueueEntry, QueueSink, QueueState};
use fc_core::volume::{BusKind, FsKind, VolumeInfo};
use fc_core::{ConflictDecision, ConflictInfo, DiskFullAction, Event, Interaction, Mode, OnConflict};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::Settings;

// ───────────────────────────── views sent to the UI ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeView {
    pub root: String,
    /// Stable id: fat32, fat, exfat, ntfs, refs, apfs, hfs, network, other.
    pub fs: String,
    pub fs_label: String,
    pub label: String,
    pub total_bytes: u64,
    /// None when unknown.
    pub free_bytes: Option<u64>,
    pub cluster_size: u64,
    pub read_only: bool,
    pub removable: bool,
    pub external: bool,
    pub network: bool,
    pub bus: BusKind,
    pub model: String,
    pub is_system: bool,
    pub disk_id: Option<String>,
    pub formattable: bool,
}

impl From<&VolumeInfo> for VolumeView {
    fn from(v: &VolumeInfo) -> Self {
        let fs = match &v.fs {
            FsKind::Fat32 => "fat32",
            FsKind::Fat => "fat",
            FsKind::ExFat => "exfat",
            FsKind::Ntfs => "ntfs",
            FsKind::ReFs => "refs",
            FsKind::Apfs => "apfs",
            FsKind::Hfs => "hfs",
            FsKind::Network => "network",
            FsKind::Other(_) => "other",
        };
        VolumeView {
            root: v.root.display().to_string(),
            fs: fs.into(),
            fs_label: v.fs.label(),
            label: v.label.clone(),
            total_bytes: v.total_bytes,
            free_bytes: (v.free_bytes != u64::MAX).then_some(v.free_bytes),
            cluster_size: v.cluster_size,
            read_only: v.read_only,
            removable: v.removable,
            external: v.is_external(),
            network: v.is_network(),
            bus: v.bus,
            model: v.model.clone(),
            is_system: v.is_system,
            disk_id: v.disk_id.clone(),
            formattable: fc_core::format::check_formattable(v).is_ok(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub id: String,
    pub sources: Vec<String>,
    pub dest: String,
    pub mode: Mode,
    pub total_bytes: u64,
    pub total_files: u64,
    pub total_dirs: u64,
    /// fc-core `Issue`s as serialized by serde: {severity, kind, …fields, message}.
    pub issues: Vec<Issue>,
    pub blocking: bool,
    pub profile: String,
    pub renamed_count: usize,
    /// First renamed paths (source-relative → destination-relative).
    pub renamed_sample: Vec<(String, String)>,
    pub same_volume: bool,
    pub src_volume: VolumeView,
    pub dst_volume: VolumeView,
}

impl From<&Plan> for PlanView {
    fn from(p: &Plan) -> Self {
        PlanView {
            id: p.id.clone(),
            sources: p.spec.sources.iter().map(|s| s.display().to_string()).collect(),
            dest: p.spec.dest.display().to_string(),
            mode: p.spec.mode,
            total_bytes: p.total_bytes,
            total_files: p.total_files,
            total_dirs: p.total_dirs,
            issues: p.issues.clone(),
            blocking: p.issues.iter().any(|i| i.severity == Severity::Error),
            profile: p.profile.name.clone(),
            renamed_count: p.renamed.len(),
            renamed_sample: p.renamed.iter().take(50).cloned().collect(),
            same_volume: p.same_volume,
            src_volume: (&p.src_volume).into(),
            dst_volume: (&p.dst_volume).into(),
        }
    }
}

/// Something the UI must act upon (shell launches, dock drops…). Drained by `take_intents`
/// so nothing is lost if it happens before the web view has loaded.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Intent {
    #[serde(rename_all = "camelCase")]
    Sources { sources: Vec<String>, mode: Option<Mode> },
    #[serde(rename_all = "camelCase")]
    Plan { plan: PlanView },
    #[serde(rename_all = "camelCase")]
    Started { job_id: String },
    #[serde(rename_all = "camelCase")]
    Error { message: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PendingRequest {
    #[serde(rename_all = "camelCase")]
    Conflict { request_id: u64, job_id: Option<String>, info: ConflictInfo },
    #[serde(rename_all = "camelCase")]
    DiskFull { request_id: u64, job_id: Option<String>, path: String, needed: u64, available: u64 },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictAnswer {
    pub action: OnConflict,
    #[serde(default)]
    pub apply_to_all: bool,
}

enum Answer {
    Conflict(ConflictDecision),
    DiskFull(DiskFullAction),
}

struct Waiting {
    req: PendingRequest,
    tx: Sender<Answer>,
}

// ───────────────────────────── interaction bridge ─────────────────────────────

pub struct Requests {
    app: AppHandle,
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, Waiting>>,
    running_job: Mutex<Option<String>>,
}

impl Requests {
    fn new(app: AppHandle) -> Self {
        Requests { app, next: AtomicU64::new(1), waiting: Mutex::new(HashMap::new()), running_job: Mutex::new(None) }
    }

    fn ask(&self, make: impl FnOnce(u64, Option<String>) -> PendingRequest, event: &str) -> Option<Answer> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let job = self.running_job.lock().unwrap().clone();
        let req = make(id, job);
        let (tx, rx) = channel();
        self.waiting.lock().unwrap().insert(id, Waiting { req: req.clone(), tx });
        crate::show_main_window(&self.app);
        let _ = self.app.emit(event, &req);
        rx.recv().ok()
    }

    pub fn pending(&self) -> Vec<PendingRequest> {
        let mut v: Vec<(u64, PendingRequest)> = self.waiting.lock().unwrap().iter().map(|(k, w)| (*k, w.req.clone())).collect();
        v.sort_by_key(|(k, _)| *k);
        v.into_iter().map(|(_, r)| r).collect()
    }

    pub fn resolve_conflict(&self, id: u64, d: ConflictDecision) -> bool {
        let mut w = self.waiting.lock().unwrap();
        let Some(entry) = w.remove(&id) else { return false };
        let _ = entry.tx.send(Answer::Conflict(d));
        if d.apply_to_all {
            // Workers may have asked concurrently: give them the same answer.
            let others: Vec<u64> = w.iter().filter(|(_, x)| matches!(x.req, PendingRequest::Conflict { .. })).map(|(k, _)| *k).collect();
            for k in others {
                if let Some(x) = w.remove(&k) {
                    let _ = x.tx.send(Answer::Conflict(d));
                    let _ = self.app.emit("request-closed", k);
                }
            }
        }
        true
    }

    pub fn resolve_disk_full(&self, id: u64, a: DiskFullAction) -> bool {
        match self.waiting.lock().unwrap().remove(&id) {
            Some(entry) => {
                let _ = entry.tx.send(Answer::DiskFull(a));
                true
            }
            None => false,
        }
    }

    /// Unblock everything (job cancelled): conflicts → skip, disk full → abort.
    pub fn release_all(&self) {
        let drained: Vec<(u64, Waiting)> = self.waiting.lock().unwrap().drain().collect();
        for (k, w) in drained {
            let a = match w.req {
                PendingRequest::Conflict { .. } => Answer::Conflict(ConflictDecision { action: OnConflict::Skip, apply_to_all: true }),
                PendingRequest::DiskFull { .. } => Answer::DiskFull(DiskFullAction::Abort),
            };
            let _ = w.tx.send(a);
            let _ = self.app.emit("request-closed", k);
        }
    }
}

pub struct GuiInteraction(pub Arc<Requests>);

impl Interaction for GuiInteraction {
    fn resolve_conflict(&self, info: &ConflictInfo) -> ConflictDecision {
        let info = info.clone();
        match self.0.ask(|request_id, job_id| PendingRequest::Conflict { request_id, job_id, info }, "conflict-request") {
            Some(Answer::Conflict(d)) if d.action != OnConflict::Ask => d,
            _ => ConflictDecision { action: OnConflict::Skip, apply_to_all: false },
        }
    }

    fn disk_full(&self, path: &str, needed: u64, available: u64) -> DiskFullAction {
        let path = path.to_string();
        match self.0.ask(|request_id, job_id| PendingRequest::DiskFull { request_id, job_id, path, needed, available }, "disk-full-request") {
            Some(Answer::DiskFull(a)) => a,
            _ => DiskFullAction::Abort,
        }
    }
}

// ───────────────────────────── event sink ─────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobEventPayload<'a> {
    job_id: &'a str,
    event: &'a Event,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FileBatch {
    job_id: String,
    /// Files committed since the previous batch.
    done: u64,
    skipped: u64,
    /// Most recent paths (at most 20).
    recent: Vec<String>,
}

#[derive(Default)]
struct JobThrottle {
    last_progress: Option<Instant>,
    last_phase: Option<fc_core::Phase>,
    last_flush: Option<Instant>,
    done: u64,
    skipped: u64,
    recent: Vec<String>,
}

const PROGRESS_MIN: Duration = Duration::from_millis(33); // ≤ 30 events/s per job
const FILES_FLUSH: Duration = Duration::from_millis(200);

pub struct TauriSink {
    app: AppHandle,
    requests: Arc<Requests>,
    throttle: Mutex<HashMap<String, JobThrottle>>,
}

impl TauriSink {
    fn flush_files(&self, job_id: &str, t: &mut JobThrottle) {
        if t.done == 0 && t.skipped == 0 {
            return;
        }
        let batch = FileBatch { job_id: job_id.to_string(), done: t.done, skipped: t.skipped, recent: std::mem::take(&mut t.recent) };
        t.done = 0;
        t.skipped = 0;
        t.last_flush = Some(Instant::now());
        let _ = self.app.emit("job-files", batch);
    }

    fn emit_event(&self, job_id: &str, e: &Event) {
        let _ = self.app.emit("job-event", JobEventPayload { job_id, event: e });
    }
}

impl QueueSink for TauriSink {
    fn job_event(&self, job_id: &str, e: &Event) {
        let mut map = self.throttle.lock().unwrap();
        let t = map.entry(job_id.to_string()).or_default();
        match e {
            Event::Progress(s) => {
                let now = Instant::now();
                let phase_changed = t.last_phase != Some(s.phase);
                if !phase_changed && t.last_progress.is_some_and(|l| now.duration_since(l) < PROGRESS_MIN) {
                    return;
                }
                t.last_progress = Some(now);
                t.last_phase = Some(s.phase);
                if t.last_flush.map_or(true, |l| now.duration_since(l) >= FILES_FLUSH) {
                    self.flush_files(job_id, t);
                }
                drop(map);
                self.emit_event(job_id, e);
            }
            Event::FileDone { path, .. } | Event::FileSkipped { path, .. } => {
                if !self.app.state::<AppState>().settings.lock().unwrap().file_events {
                    return;
                }
                if matches!(e, Event::FileDone { .. }) {
                    t.done += 1;
                } else {
                    t.skipped += 1;
                }
                t.recent.push(path.clone());
                if t.recent.len() > 20 {
                    t.recent.remove(0);
                }
            }
            Event::Planned { .. } => {
                *self.requests.running_job.lock().unwrap() = Some(job_id.to_string());
                drop(map);
                self.emit_event(job_id, e);
            }
            Event::Finished(report) => {
                self.flush_files(job_id, t);
                map.remove(job_id);
                drop(map);
                *self.requests.running_job.lock().unwrap() = None;
                self.emit_event(job_id, e);
                crate::notify_finished(&self.app, report);
            }
            _ => {
                drop(map);
                self.emit_event(job_id, e);
            }
        }
    }

    fn queue_changed(&self, entries: Vec<QueueEntry>) {
        let _ = self.app.emit("queue-changed", entries);
    }
}

// ───────────────────────────── app state ─────────────────────────────

pub struct AppState {
    pub queue: JobQueue,
    pub plans: Mutex<HashMap<String, Plan>>,
    pub requests: Arc<Requests>,
    pub settings: Mutex<Settings>,
    pub intents: Mutex<Vec<Intent>>,
    pub effect: Mutex<&'static str>,
}

impl AppState {
    pub fn new(app: &AppHandle) -> AppState {
        let requests = Arc::new(Requests::new(app.clone()));
        let sink = Arc::new(TauriSink { app: app.clone(), requests: requests.clone(), throttle: Mutex::new(HashMap::new()) });
        let queue = JobQueue::new(sink, Arc::new(GuiInteraction(requests.clone())));
        AppState {
            queue,
            plans: Mutex::new(HashMap::new()),
            requests,
            settings: Mutex::new(Settings::load()),
            intents: Mutex::new(Vec::new()),
            effect: Mutex::new("none"),
        }
    }

    pub fn lang(&self) -> String {
        self.settings.lock().unwrap().resolved_lang()
    }

    /// Jobs not finished yet (pending, running, paused).
    pub fn active_jobs(&self) -> usize {
        self.queue.list().iter().filter(|e| !matches!(e.state, QueueState::Finished(_))).count()
    }

    pub fn push_intent(&self, app: &AppHandle, i: Intent) {
        self.intents.lock().unwrap().push(i);
        let _ = app.emit("intent", ());
    }

    /// Keep a built plan for `start_plan`; returns its UI view.
    pub fn keep_plan(&self, plan: Plan) -> PlanView {
        let view = PlanView::from(&plan);
        let mut plans = self.plans.lock().unwrap();
        if plans.len() > 32 {
            plans.clear();
        }
        plans.insert(plan.id.clone(), plan);
        view
    }

    pub fn spec_with_defaults(&self, sources: Vec<PathBuf>, dest: PathBuf, mode: Mode) -> fc_core::JobSpec {
        let s = self.settings.lock().unwrap();
        let mut spec = fc_core::JobSpec::new(sources, dest);
        spec.mode = mode;
        spec.verify = s.verify;
        spec.on_conflict = s.on_conflict;
        spec.skip_junk = s.skip_junk;
        spec
    }
}
