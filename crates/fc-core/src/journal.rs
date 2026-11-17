//! Crash-safe job journal (append-only JSON lines) enabling automatic resume.
//!
//! Records are appended as work progresses; a file is only marked `done` after the
//! data covering it has been flushed to the device. On resume, `done` files are
//! skipped, large files continue from their last durable checkpoint (after
//! re-validating the already-written prefix), and anything uncertain is re-verified.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{Error, IoContext, Result};
use crate::plan::Plan;
use crate::types::OnConflict;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Record {
    /// Full plan (first record).
    Plan { plan: Box<Plan>, created_ms: i64 },
    /// Conflict decision taken for item `i` (before acting on it).
    Decide { i: usize, action: OnConflict },
    /// About to commit (rename into place) item `i`.
    Intent { i: usize },
    /// Durable checkpoint: first `off` bytes of item `i`'s temp file are on disk.
    Ckpt { i: usize, off: u64 },
    /// Item `i` committed and durable.
    Done { i: usize },
    /// Item skipped (conflict policy / junk).
    Skip { i: usize },
    Failed { i: usize, error: String },
    /// Job ended.
    End { status: String },
}

/// Reconstructed state of an interrupted job.
#[derive(Debug, Default, Clone)]
pub struct ResumeState {
    pub done: HashSet<usize>,
    pub skipped: HashSet<usize>,
    pub decisions: HashMap<usize, OnConflict>,
    pub intents: HashSet<usize>,
    pub checkpoints: HashMap<usize, u64>,
    pub ended: Option<String>,
}

pub struct Journal {
    path: PathBuf,
    w: Mutex<BufWriter<File>>,
}

/// Where job journals live: `<local data>/FastCopy/jobs`.
pub fn jobs_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("FastCopy").join("jobs")
}

pub fn journal_path(job_id: &str) -> PathBuf {
    jobs_dir().join(format!("{job_id}.jsonl"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Journal {
    /// Start a new journal for `plan`.
    pub fn create(plan: &Plan) -> Result<Journal> {
        Self::create_in(&jobs_dir(), plan)
    }

    pub fn create_in(dir: &Path, plan: &Plan) -> Result<Journal> {
        std::fs::create_dir_all(dir).ctx("création du dossier des tâches", dir)?;
        let path = dir.join(format!("{}.jsonl", plan.id));
        let f = OpenOptions::new().create(true).truncate(true).write(true).open(&path).ctx("création du journal", &path)?;
        let j = Journal { path, w: Mutex::new(BufWriter::with_capacity(256 * 1024, f)) };
        j.append(&Record::Plan { plan: Box::new(plan.clone()), created_ms: now_ms() })?;
        j.sync()?;
        Ok(j)
    }

    /// Re-open an existing journal for appending.
    pub fn reopen(path: &Path) -> Result<Journal> {
        let f = OpenOptions::new().append(true).open(path).ctx("ouverture du journal", path)?;
        Ok(Journal { path: path.to_path_buf(), w: Mutex::new(BufWriter::with_capacity(256 * 1024, f)) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, r: &Record) -> Result<()> {
        let mut line = serde_json::to_vec(r).map_err(|e| Error::Journal(e.to_string()))?;
        line.push(b'\n');
        self.w.lock().unwrap().write_all(&line).map_err(|e| Error::Journal(e.to_string()))
    }

    /// Flush buffered records to the OS (cheap; durability comes with `sync`).
    pub fn flush(&self) -> Result<()> {
        self.w.lock().unwrap().flush().map_err(|e| Error::Journal(e.to_string()))
    }

    /// Make every appended record durable.
    pub fn sync(&self) -> Result<()> {
        let mut w = self.w.lock().unwrap();
        w.flush().map_err(|e| Error::Journal(e.to_string()))?;
        w.get_ref().sync_data().map_err(|e| Error::Journal(e.to_string()))
    }

    /// Load a journal: plan + state. Tolerates a truncated last line.
    pub fn load(path: &Path) -> Result<(Plan, ResumeState)> {
        let f = File::open(path).ctx("ouverture du journal", path)?;
        let mut plan: Option<Plan> = None;
        let mut st = ResumeState::default();
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<Record>(&line) else { break };
            match rec {
                Record::Plan { plan: p, .. } => plan = Some(*p),
                Record::Decide { i, action } => {
                    st.decisions.insert(i, action);
                }
                Record::Intent { i } => {
                    st.intents.insert(i);
                }
                Record::Ckpt { i, off } => {
                    st.checkpoints.insert(i, off);
                }
                Record::Done { i } => {
                    st.done.insert(i);
                    st.checkpoints.remove(&i);
                }
                Record::Skip { i } => {
                    st.skipped.insert(i);
                }
                Record::Failed { .. } => {}
                Record::End { status } => st.ended = Some(status),
            }
        }
        let plan = plan.ok_or_else(|| Error::Journal(format!("journal vide ou corrompu : {}", path.display())))?;
        Ok((plan, st))
    }
}

/// Summary of a stored job, for `fcopy jobs` and the GUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: String,
    pub path: PathBuf,
    pub sources: Vec<PathBuf>,
    pub dest: PathBuf,
    pub total_files: u64,
    pub done_files: u64,
    pub total_bytes: u64,
    pub ended: Option<String>,
    pub modified_ms: i64,
}

impl JobSummary {
    pub fn resumable(&self) -> bool {
        !matches!(self.ended.as_deref(), Some("completed"))
    }
}

pub fn list_jobs() -> Vec<JobSummary> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(jobs_dir()) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok((plan, st)) = Journal::load(&p) {
            let modified_ms = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let file_items: HashSet<usize> = plan
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| !matches!(it.kind, crate::plan::ItemKind::Dir))
                .map(|(i, _)| i)
                .collect();
            out.push(JobSummary {
                id: plan.id.clone(),
                path: p,
                sources: plan.spec.sources.clone(),
                dest: plan.spec.dest.clone(),
                total_files: plan.total_files,
                done_files: st.done.iter().chain(st.skipped.iter()).filter(|i| file_items.contains(i)).count() as u64,
                total_bytes: plan.total_bytes,
                ended: st.ended,
                modified_ms,
            });
        }
    }
    out.sort_by_key(|j| std::cmp::Reverse(j.modified_ms));
    out
}

/// Remove a job journal and its leftover temporary files.
pub fn discard_job(id: &str) -> Result<usize> {
    let path = journal_path(id);
    let (plan, _) = Journal::load(&path)?;
    let mut removed = 0;
    for it in &plan.items {
        let tmp = crate::util::temp_path(&plan.dst_path(it));
        if tmp.exists() && std::fs::remove_file(&tmp).is_ok() {
            removed += 1;
        }
    }
    std::fs::remove_file(&path).ctx("suppression du journal", &path)?;
    Ok(removed)
}
