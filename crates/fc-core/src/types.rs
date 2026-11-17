//! Public types shared by the CLI and GUI: job spec, events, control, interaction.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Copy,
    Move,
}

/// How strongly copies are verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VerifyMode {
    /// On-the-fly hash only, no re-read of the destination.
    Fast,
    /// Re-read the destination bypassing caches and compare (default).
    #[default]
    Full,
    /// Full + re-read the source too.
    Paranoid,
}

/// What to do when the destination file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OnConflict {
    #[default]
    Ask,
    Overwrite,
    Skip,
    KeepBoth,
    /// Overwrite only if the source is newer.
    Newer,
    /// Overwrite only if size or date differ.
    IfDifferent,
}

/// What to do with files too large for the destination filesystem (FAT32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SplitMode {
    /// Split oversize files into `.001`, `.002`… chunks.
    #[default]
    Auto,
    /// Refuse (plan reports a blocking issue).
    Never,
}

/// Everything needed to describe a copy job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub sources: Vec<PathBuf>,
    pub dest: PathBuf,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub verify: VerifyMode,
    #[serde(default)]
    pub on_conflict: OnConflict,
    #[serde(default)]
    pub split: SplitMode,
    /// Skip `.DS_Store`, `._*`, `Thumbs.db`…
    #[serde(default = "yes")]
    pub skip_junk: bool,
    /// Copy locked files through a VSS snapshot (Windows, admin).
    #[serde(default)]
    pub use_vss: bool,
    /// Override the number of small-file workers.
    #[serde(default)]
    pub threads: Option<usize>,
    /// Reassemble `.fcsplit.json` chunk sets found in the sources.
    #[serde(default = "yes")]
    pub auto_join: bool,
    /// Testing / advanced: override the destination's max file size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_size_override: Option<u64>,
    /// Testing / advanced: override the split chunk size (multiple of 4096).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_chunk_override: Option<u64>,
}

fn yes() -> bool {
    true
}

impl JobSpec {
    pub fn new(sources: Vec<PathBuf>, dest: PathBuf) -> Self {
        JobSpec {
            sources,
            dest,
            mode: Mode::Copy,
            verify: VerifyMode::Full,
            on_conflict: OnConflict::Ask,
            split: SplitMode::Auto,
            skip_junk: true,
            use_vss: false,
            threads: None,
            auto_join: true,
            max_file_size_override: None,
            split_chunk_override: None,
        }
    }
}

/// Pause / resume / cancel handle shared with running jobs.
#[derive(Default)]
pub struct Control {
    paused: Mutex<bool>,
    cv: Condvar,
    cancelled: AtomicBool,
}

impl Control {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn pause(&self) {
        *self.paused.lock().unwrap() = true;
    }
    pub fn resume(&self) {
        *self.paused.lock().unwrap() = false;
        self.cv.notify_all();
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.resume();
    }
    pub fn is_paused(&self) -> bool {
        *self.paused.lock().unwrap()
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    /// Block while paused; error if cancelled. Called between chunks and files.
    pub fn checkpoint(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut p = self.paused.lock().unwrap();
        while *p && !self.is_cancelled() {
            p = self.cv.wait(p).unwrap();
        }
        drop(p);
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Phase {
    Planning = 0,
    Copying = 1,
    Verifying = 2,
    Finalizing = 3,
    Done = 4,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            0 => Phase::Planning,
            1 => Phase::Copying,
            2 => Phase::Verifying,
            3 => Phase::Finalizing,
            _ => Phase::Done,
        }
    }
}

/// Live counters, updated lock-free by workers.
pub struct Stats {
    pub start: Instant,
    pub bytes_total: AtomicU64,
    pub bytes_done: AtomicU64,
    pub bytes_verified: AtomicU64,
    pub files_total: AtomicU64,
    pub files_done: AtomicU64,
    pub files_skipped: AtomicU64,
    pub files_failed: AtomicU64,
    phase: AtomicU8,
    pub current: Mutex<String>,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            start: Instant::now(),
            bytes_total: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            bytes_verified: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            files_skipped: AtomicU64::new(0),
            files_failed: AtomicU64::new(0),
            phase: AtomicU8::new(0),
            current: Mutex::new(String::new()),
        }
    }
}

impl Stats {
    pub fn set_phase(&self, p: Phase) {
        self.phase.store(p as u8, Ordering::Relaxed);
    }
    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Relaxed))
    }
    pub fn add_done(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
    }
    pub fn add_verified(&self, n: u64) {
        self.bytes_verified.fetch_add(n, Ordering::Relaxed);
    }
    pub fn set_current(&self, s: &str) {
        let mut c = self.current.lock().unwrap();
        c.clear();
        c.push_str(s);
    }
}

/// A point-in-time view of progress, sent to UIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub phase: Phase,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub bytes_verified: u64,
    pub files_total: u64,
    pub files_done: u64,
    pub files_skipped: u64,
    pub files_failed: u64,
    pub current: String,
    /// Bytes per second (smoothed).
    pub speed: f64,
    pub elapsed_ms: u64,
    pub eta_secs: Option<u64>,
    pub paused: bool,
}

/// Final result of a job.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Report {
    pub job_id: String,
    pub status: JobStatus,
    pub files_copied: u64,
    pub files_skipped: u64,
    pub files_failed: u64,
    pub bytes_copied: u64,
    pub elapsed_ms: u64,
    pub avg_speed: f64,
    pub errors: Vec<FileError>,
    pub renamed: Vec<(String, String)>,
    pub warnings: Vec<String>,
    /// Files that could not be opened because they are in use (candidates for VSS).
    pub locked: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum JobStatus {
    #[default]
    Completed,
    CompletedWithErrors,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileError {
    pub path: String,
    pub error: String,
}

/// Events streamed to the UI / CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    Planned { files: u64, bytes: u64, dirs: u64 },
    Progress(Snapshot),
    FileDone { path: String, bytes: u64 },
    FileSkipped { path: String, reason: String },
    FileFailed { path: String, error: String },
    Warning { message: String },
    Finished(Report),
}

pub trait EventSink: Send + Sync {
    fn event(&self, e: Event);
}

impl<F: Fn(Event) + Send + Sync> EventSink for F {
    fn event(&self, e: Event) {
        self(e)
    }
}

/// Sink that drops everything.
pub struct NullSink;
impl EventSink for NullSink {
    fn event(&self, _e: Event) {}
}

/// Description of a conflict shown to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub path: String,
    pub src_size: u64,
    pub src_mtime_ms: i64,
    pub dst_size: u64,
    pub dst_mtime_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictDecision {
    /// Never `Ask`.
    pub action: OnConflict,
    pub apply_to_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskFullAction {
    Retry,
    Skip,
    Abort,
}

/// Questions the engine may need to ask the user while running.
pub trait Interaction: Send + Sync {
    fn resolve_conflict(&self, info: &ConflictInfo) -> ConflictDecision;
    fn disk_full(&self, path: &str, needed: u64, available: u64) -> DiskFullAction;
}

/// Non-interactive answers: conflicts → skip, disk full → abort.
pub struct NonInteractive;
impl Interaction for NonInteractive {
    fn resolve_conflict(&self, _info: &ConflictInfo) -> ConflictDecision {
        ConflictDecision { action: OnConflict::Skip, apply_to_all: true }
    }
    fn disk_full(&self, _path: &str, _needed: u64, _available: u64) -> DiskFullAction {
        DiskFullAction::Abort
    }
}
