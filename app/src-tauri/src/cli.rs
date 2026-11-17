//! Command-line / shell-integration requests.
//!
//! * `--shell copy|move <paths…>` — remember the selection ("paste here with Fast Copy").
//! * `--shell paste <dir>` — paste the remembered (or Explorer-copied) files into `<dir>`.
//! * `--shell sendto <paths…>` — open the window with these sources.
//! * bare paths — same as `sendto`.
//!
//! Any path may be `@<listfile>` (UTF-8, one path per line). Explorer starts one process
//! per selected item for registry verbs, so requests with the same verb arriving within
//! [`AGGREGATE`] are merged into one selection.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use fc_core::Mode;
use tauri::{AppHandle, Manager};

use crate::state::{AppState, Intent};

pub const AGGREGATE: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Copy,
    Move,
    Paste,
    SendTo,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub verb: Verb,
    pub paths: Vec<PathBuf>,
}

/// Parse a full argv (program name first). Returns None when there is nothing to do.
pub fn parse(argv: &[String], cwd: Option<&Path>) -> Option<Request> {
    let args: Vec<&String> = argv.iter().skip(1).collect();
    let (verb, rest): (Verb, Vec<&String>) = match args.iter().position(|a| *a == "--shell") {
        Some(i) => {
            let verb = match args.get(i + 1).map(|s| s.as_str()) {
                Some("copy") => Verb::Copy,
                Some("move") => Verb::Move,
                Some("paste") | Some("paste-bg") => Verb::Paste,
                Some("sendto") | Some("open") => Verb::SendTo,
                _ => return None,
            };
            (verb, args.iter().skip(i + 2).copied().collect())
        }
        None => (
            Verb::SendTo,
            args.iter()
                .copied()
                .filter(|a| !a.starts_with("--") && !a.starts_with("-psn_"))
                .collect(),
        ),
    };
    let paths = expand(rest.iter().map(|s| s.as_str()), cwd);
    if paths.is_empty() && verb != Verb::Paste {
        return None;
    }
    Some(Request { verb, paths })
}

/// Expand `@listfile` arguments and make relative paths absolute.
pub fn expand<'a>(args: impl Iterator<Item = &'a str>, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in args {
        let a = a.trim().trim_matches('"');
        if a.is_empty() {
            continue;
        }
        if let Some(list) = a.strip_prefix('@') {
            let lp = absolute(Path::new(list), cwd);
            if let Ok(text) = std::fs::read_to_string(&lp) {
                for line in text.trim_start_matches('\u{feff}').lines() {
                    let line = line.trim().trim_matches('"');
                    if !line.is_empty() {
                        out.push(absolute(Path::new(line), cwd));
                    }
                }
                // Lists written by the shell extension (%TEMP%astcopy-shell-<pid>-<n>.lst) are
                // ours to delete once read. Never delete a user file outside the temp folder.
                if is_in_temp(&lp) {
                    let _ = std::fs::remove_file(&lp);
                }
                continue;
            }
        }
        out.push(absolute(Path::new(a), cwd));
    }
    out
}

fn is_in_temp(p: &Path) -> bool {
    let canon = |x: &Path| x.canonicalize().unwrap_or_else(|_| x.to_path_buf());
    let tmp = canon(&std::env::temp_dir());
    canon(p).starts_with(&tmp)
}

fn absolute(p: &Path, cwd: Option<&Path>) -> PathBuf {
    if p.is_absolute() {
        return p.to_path_buf();
    }
    match cwd {
        Some(c) => c.join(p),
        None => std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf()),
    }
}

// ───────────────────────────── aggregation ─────────────────────────────

#[derive(Default)]
pub struct Aggregator {
    pending: Mutex<Option<(u64, Request)>>,
    gen: Mutex<u64>,
}

impl Aggregator {
    pub fn submit(&self, app: &AppHandle, req: Request) {
        let mut flush_now = None;
        let gen = {
            let mut g = self.gen.lock().unwrap();
            *g += 1;
            *g
        };
        {
            let mut p = self.pending.lock().unwrap();
            match p.as_mut() {
                Some((_, cur)) if cur.verb == req.verb => {
                    for path in req.paths {
                        if !cur.paths.contains(&path) {
                            cur.paths.push(path);
                        }
                    }
                    p.as_mut().unwrap().0 = gen;
                }
                _ => {
                    flush_now = p.take().map(|(_, r)| r);
                    *p = Some((gen, req));
                }
            }
        }
        if let Some(r) = flush_now {
            dispatch(app, r);
        }
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(AGGREGATE);
            let agg = app.state::<Aggregator>();
            let ready = {
                let mut p = agg.pending.lock().unwrap();
                if p.as_ref().is_some_and(|(g, _)| *g == gen) {
                    p.take().map(|(_, r)| r)
                } else {
                    None
                }
            };
            if let Some(r) = ready {
                dispatch(&app, r);
            }
        });
    }
}

fn strings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.display().to_string()).collect()
}

/// Act on a (merged) request.
pub fn dispatch(app: &AppHandle, req: Request) {
    let state = app.state::<AppState>();
    match req.verb {
        Verb::Copy | Verb::Move => {
            let mode = if req.verb == Verb::Move {
                Mode::Move
            } else {
                Mode::Copy
            };
            if let Err(e) = fc_core::shell::set_app_clipboard(mode, req.paths.clone()) {
                state.push_intent(
                    app,
                    Intent::Error {
                        message: e.to_string(),
                    },
                );
            } else {
                let (title, body) =
                    crate::texts::items_ready(&state.lang(), req.paths.len(), mode == Mode::Move);
                crate::notify(app, &title, &body);
            }
            state.push_intent(
                app,
                Intent::Sources {
                    sources: strings(&req.paths),
                    mode: Some(mode),
                },
            );
            crate::show_main_window(app);
        }
        Verb::SendTo => {
            state.push_intent(
                app,
                Intent::Sources {
                    sources: strings(&req.paths),
                    mode: None,
                },
            );
            crate::show_main_window(app);
        }
        Verb::Paste => {
            crate::show_main_window(app);
            let Some(dest) = req.paths.first().cloned() else {
                return;
            };
            let Some((sources, mode)) = fc_core::shell::paste_source() else {
                let msg = if state.lang() == "fr" {
                    "Rien à coller : utilisez d'abord « Copier avec Fast Copy »."
                } else {
                    "Nothing to paste: use “Copy with Fast Copy” first."
                };
                state.push_intent(
                    app,
                    Intent::Error {
                        message: msg.into(),
                    },
                );
                return;
            };
            let spec = state.spec_with_defaults(sources, dest, mode);
            let app = app.clone();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                match fc_core::plan::build(&spec) {
                    Ok(plan) if plan.blocking_issues().is_empty() => {
                        let id = state.queue.enqueue(plan, false);
                        if mode == Mode::Move {
                            fc_core::shell::clear_app_clipboard();
                        }
                        state.push_intent(&app, Intent::Started { job_id: id });
                    }
                    Ok(plan) => {
                        let view = state.keep_plan(plan);
                        state.push_intent(
                            &app,
                            Intent::Plan {
                                plan: Box::new(view),
                            },
                        );
                    }
                    Err(e) => state.push_intent(
                        &app,
                        Intent::Error {
                            message: e.to_string(),
                        },
                    ),
                }
            });
        }
    }
}
