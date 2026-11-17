//! Job queue: jobs run one after another; pending jobs can be reordered, paused,
//! resumed or cancelled while another one runs.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use crate::job::{execute, ExecOptions};
use crate::journal::ResumeState;
use crate::plan::Plan;
use crate::types::{Control, Event, EventSink, Interaction, JobStatus, Report};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueState {
    Pending,
    Running,
    Paused,
    Finished(JobStatus),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: String,
    pub state: QueueState,
    pub sources: Vec<String>,
    pub dest: String,
    pub total_bytes: u64,
    pub total_files: u64,
    pub mode: crate::types::Mode,
    pub report: Option<Report>,
}

pub trait QueueSink: Send + Sync {
    fn job_event(&self, job_id: &str, e: &Event);
    fn queue_changed(&self, entries: Vec<QueueEntry>);
}

struct Job {
    plan: Plan,
    control: Arc<Control>,
    state: QueueState,
    report: Option<Report>,
    force: bool,
    resume: Option<ResumeState>,
}

struct Inner {
    jobs: Mutex<VecDeque<Job>>,
    cv: Condvar,
    sink: Arc<dyn QueueSink>,
    ui: Arc<dyn Interaction>,
}

#[derive(Clone)]
pub struct JobQueue {
    inner: Arc<Inner>,
}

struct Forward<'a> {
    id: String,
    sink: &'a dyn QueueSink,
}
impl EventSink for Forward<'_> {
    fn event(&self, e: Event) {
        self.sink.job_event(&self.id, &e);
    }
}

impl JobQueue {
    pub fn new(sink: Arc<dyn QueueSink>, ui: Arc<dyn Interaction>) -> JobQueue {
        let inner = Arc::new(Inner {
            jobs: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            sink,
            ui,
        });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("fc-queue".into())
            .spawn(move || worker_loop(worker))
            .expect("spawn queue worker");
        JobQueue { inner }
    }

    pub fn enqueue(&self, plan: Plan, force: bool) -> String {
        self.push(plan, force, None)
    }

    /// Enqueue an interrupted job recovered from its journal (see [`crate::journal::Journal::load`]).
    /// The plan was accepted when the job first started, so it always runs forced.
    pub fn enqueue_resume(&self, plan: Plan, state: ResumeState) -> String {
        self.push(plan, true, Some(state))
    }

    fn push(&self, plan: Plan, force: bool, resume: Option<ResumeState>) -> String {
        let id = plan.id.clone();
        self.inner.jobs.lock().unwrap().push_back(Job {
            plan,
            control: Arc::new(Control::new()),
            state: QueueState::Pending,
            report: None,
            force,
            resume,
        });
        self.changed();
        self.inner.cv.notify_all();
        id
    }

    pub fn list(&self) -> Vec<QueueEntry> {
        entries(&self.inner.jobs.lock().unwrap())
    }

    fn with_job<R>(&self, id: &str, f: impl FnOnce(&mut Job) -> R) -> Option<R> {
        let mut jobs = self.inner.jobs.lock().unwrap();
        let r = jobs.iter_mut().find(|j| j.plan.id == id).map(f);
        drop(jobs);
        self.changed();
        self.inner.cv.notify_all();
        r
    }

    pub fn pause(&self, id: &str) {
        self.with_job(id, |j| match j.state {
            QueueState::Running => {
                j.control.pause();
                j.state = QueueState::Paused;
            }
            QueueState::Pending => j.state = QueueState::Paused,
            _ => {}
        });
    }

    pub fn resume(&self, id: &str) {
        self.with_job(id, |j| {
            if j.state == QueueState::Paused {
                if j.control.is_paused() {
                    j.control.resume();
                    j.state = QueueState::Running;
                } else {
                    j.state = QueueState::Pending;
                }
            }
        });
    }

    pub fn cancel(&self, id: &str) {
        self.with_job(id, |j| match j.state {
            QueueState::Running | QueueState::Paused
                if j.control.is_paused() || j.state == QueueState::Running =>
            {
                j.control.cancel()
            }
            QueueState::Pending | QueueState::Paused => {
                j.state = QueueState::Finished(JobStatus::Cancelled)
            }
            _ => {}
        });
    }

    /// Move a pending job by `delta` positions (negative = earlier).
    pub fn reorder(&self, id: &str, delta: i32) {
        let mut jobs = self.inner.jobs.lock().unwrap();
        if let Some(pos) = jobs.iter().position(|j| j.plan.id == id) {
            let new = (pos as i64 + delta as i64).clamp(0, jobs.len() as i64 - 1) as usize;
            if let Some(job) = jobs.remove(pos) {
                jobs.insert(new, job);
            }
        }
        drop(jobs);
        self.changed();
    }

    /// Remove finished jobs from the list.
    pub fn clear_finished(&self) {
        self.inner
            .jobs
            .lock()
            .unwrap()
            .retain(|j| !matches!(j.state, QueueState::Finished(_)));
        self.changed();
    }

    fn changed(&self) {
        let e = self.list();
        self.inner.sink.queue_changed(e);
    }
}

fn entries(jobs: &VecDeque<Job>) -> Vec<QueueEntry> {
    jobs.iter()
        .map(|j| QueueEntry {
            id: j.plan.id.clone(),
            state: j.state,
            sources: j
                .plan
                .spec
                .sources
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            dest: j.plan.spec.dest.display().to_string(),
            total_bytes: j.plan.total_bytes,
            total_files: j.plan.total_files,
            mode: j.plan.spec.mode,
            report: j.report.clone(),
        })
        .collect()
}

fn worker_loop(inner: Arc<Inner>) {
    loop {
        let (plan, control, force, resume) = {
            let mut jobs = inner.jobs.lock().unwrap();
            loop {
                if let Some(j) = jobs.iter_mut().find(|j| j.state == QueueState::Pending) {
                    j.state = QueueState::Running;
                    break (j.plan.clone(), j.control.clone(), j.force, j.resume.take());
                }
                jobs = inner.cv.wait(jobs).unwrap();
            }
        };
        inner
            .sink
            .queue_changed(entries(&inner.jobs.lock().unwrap()));
        let fwd = Forward {
            id: plan.id.clone(),
            sink: &*inner.sink,
        };
        let opts = ExecOptions {
            file_events: true,
            force,
            resume,
            ..Default::default()
        };
        let result = execute(&plan, &control, &fwd, &*inner.ui, opts);
        let (state, report) = match result {
            Ok(r) => (QueueState::Finished(r.status), Some(r)),
            Err(e) => {
                let r = Report {
                    job_id: plan.id.clone(),
                    status: JobStatus::Failed,
                    errors: vec![crate::types::FileError {
                        path: String::new(),
                        error: e.to_string(),
                    }],
                    ..Default::default()
                };
                inner.sink.job_event(&plan.id, &Event::Finished(r.clone()));
                (QueueState::Finished(JobStatus::Failed), Some(r))
            }
        };
        {
            let mut jobs = inner.jobs.lock().unwrap();
            if let Some(j) = jobs.iter_mut().find(|j| j.plan.id == plan.id) {
                j.state = state;
                j.report = report;
            }
        }
        inner
            .sink
            .queue_changed(entries(&inner.jobs.lock().unwrap()));
    }
}
