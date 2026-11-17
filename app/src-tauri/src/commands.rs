//! Commands invoked from the web UI.

use std::path::PathBuf;

use fc_core::format::FormatRequest;
use fc_core::journal::{self, JobSummary};
use fc_core::queue::QueueEntry;
use fc_core::{ConflictDecision, DiskFullAction, JobSpec, Mode, OnConflict, SplitMode, VerifyMode};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::settings::Settings;
use crate::state::{AppState, ConflictAnswer, Intent, PendingRequest, PlanView, VolumeView};

type CmdResult<T> = Result<T, String>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecInput {
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
    #[serde(default = "yes")]
    pub skip_junk: bool,
}

fn yes() -> bool {
    true
}

/// Run blocking work (disk queries, journals, child processes) off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_volumes() -> CmdResult<Vec<VolumeView>> {
    blocking(|| Ok(fc_core::volume::list_volumes().iter().map(VolumeView::from).collect())).await
}

#[tauri::command]
pub async fn volume_info(path: PathBuf) -> CmdResult<VolumeView> {
    blocking(move || fc_core::volume::volume_info(&path).map(|v| VolumeView::from(&v)).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub async fn plan_job(state: State<'_, AppState>, spec: SpecInput) -> CmdResult<PlanView> {
    let mut s = JobSpec::new(spec.sources, spec.dest);
    s.mode = spec.mode;
    s.verify = spec.verify;
    s.on_conflict = spec.on_conflict;
    s.split = spec.split;
    s.skip_junk = spec.skip_junk;
    // Walking big trees takes time: keep the async runtime free.
    let plan = blocking(move || fc_core::plan::build(&s).map_err(|e| e.to_string())).await?;
    Ok(state.keep_plan(plan))
}

#[tauri::command]
pub fn start_plan(state: State<'_, AppState>, plan_id: String, force: bool) -> CmdResult<String> {
    let plan = state.plans.lock().unwrap().remove(&plan_id).ok_or_else(|| "plan expired".to_string())?;
    if !force && !plan.blocking_issues().is_empty() {
        let msg = plan.blocking_issues().iter().map(|i| i.message.clone()).collect::<Vec<_>>().join(" ; ");
        state.plans.lock().unwrap().insert(plan_id, plan);
        return Err(msg);
    }
    Ok(state.queue.enqueue(plan, force))
}

#[tauri::command]
pub fn pause_job(state: State<'_, AppState>, id: String) {
    state.queue.pause(&id);
}

#[tauri::command]
pub fn resume_job(state: State<'_, AppState>, id: String) {
    state.queue.resume(&id);
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, id: String) {
    let running = state.queue.list().iter().any(|e| e.id == id && matches!(e.state, fc_core::queue::QueueState::Running | fc_core::queue::QueueState::Paused));
    state.queue.cancel(&id);
    if running {
        state.requests.release_all();
    }
}

#[tauri::command]
pub fn reorder_job(state: State<'_, AppState>, id: String, delta: i32) {
    state.queue.reorder(&id, delta);
}

#[tauri::command]
pub fn clear_finished(state: State<'_, AppState>) {
    state.queue.clear_finished();
}

#[tauri::command]
pub fn list_queue(state: State<'_, AppState>) -> Vec<QueueEntry> {
    state.queue.list()
}

#[tauri::command]
pub fn resolve_conflict(state: State<'_, AppState>, request_id: u64, decision: ConflictAnswer) -> bool {
    let action = if decision.action == OnConflict::Ask { OnConflict::Skip } else { decision.action };
    state.requests.resolve_conflict(request_id, ConflictDecision { action, apply_to_all: decision.apply_to_all })
}

#[tauri::command]
pub fn resolve_disk_full(state: State<'_, AppState>, request_id: u64, action: DiskFullAction) -> bool {
    state.requests.resolve_disk_full(request_id, action)
}

#[tauri::command]
pub fn pending_requests(state: State<'_, AppState>) -> Vec<PendingRequest> {
    state.requests.pending()
}

#[tauri::command]
pub async fn format_drive(request: FormatRequest) -> CmdResult<VolumeView> {
    blocking(move || fc_core::format::format_exfat(&request).map(|v| VolumeView::from(&v)).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub async fn eject_volume(path: PathBuf) -> CmdResult<()> {
    blocking(move || fc_core::volume::eject(&path).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub fn shell_status() -> bool {
    fc_core::shell::is_installed()
}

/// Remember whether the user wants the context menus (drives the automatic install at startup).
fn set_auto_shell(state: &AppState, on: bool) {
    let mut s = state.settings.lock().unwrap();
    if s.auto_shell != on {
        s.auto_shell = on;
        let _ = s.save();
    }
}

#[tauri::command]
pub async fn shell_install(state: State<'_, AppState>) -> CmdResult<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let lang = state.lang();
    blocking(move || fc_core::shell::install(&exe, &lang).map_err(|e| e.to_string())).await?;
    set_auto_shell(&state, true);
    Ok(())
}

#[tauri::command]
pub async fn shell_uninstall(state: State<'_, AppState>) -> CmdResult<()> {
    blocking(|| fc_core::shell::uninstall().map_err(|e| e.to_string())).await?;
    set_auto_shell(&state, false);
    Ok(())
}

#[derive(Serialize)]
pub struct PasteSource {
    paths: Vec<String>,
    mode: Mode,
}

#[tauri::command]
pub fn paste_source() -> Option<PasteSource> {
    fc_core::shell::paste_source().map(|(p, mode)| PasteSource { paths: p.iter().map(|x| x.display().to_string()).collect(), mode })
}

/// Interrupted jobs that can be resumed (excluding those currently in the queue).
#[tauri::command]
pub async fn list_jobs(state: State<'_, AppState>) -> CmdResult<Vec<JobSummary>> {
    let queued: Vec<String> = state.queue.list().into_iter().map(|e| e.id).collect();
    blocking(move || Ok(journal::list_jobs().into_iter().filter(|j| j.resumable() && !queued.contains(&j.id)).collect())).await
}

#[tauri::command]
pub async fn resume_stored_job(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    if state.queue.list().iter().any(|e| e.id == id) {
        return Err("already queued".into());
    }
    let path = journal::journal_path(&id);
    let (plan, st) = blocking(move || journal::Journal::load(&path).map_err(|e| e.to_string())).await?;
    Ok(state.queue.enqueue_resume(plan, st))
}

#[tauri::command]
pub async fn discard_stored_job(id: String) -> CmdResult<usize> {
    blocking(move || journal::discard_job(&id).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_settings(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    settings.save()?;
    let lang_changed = state.settings.lock().unwrap().lang != settings.lang;
    *state.settings.lock().unwrap() = settings;
    if lang_changed {
        crate::rebuild_tray_menu(&app);
        // Keep the context-menu labels in the new language (spawns a process: not on the UI thread).
        let lang = state.lang();
        std::thread::spawn(move || {
            if fc_core::shell::is_installed() {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = fc_core::shell::install(&exe, &lang);
                }
            }
        });
    }
    Ok(())
}

#[tauri::command]
pub fn system_lang() -> String {
    fc_core::shell::system_lang()
}

#[tauri::command]
pub fn take_intents(state: State<'_, AppState>) -> Vec<Intent> {
    std::mem::take(&mut *state.intents.lock().unwrap())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    core_version: &'static str,
    platform: &'static str,
    /// Window material actually applied: mica, acrylic, vibrancy or none.
    effect: &'static str,
    lang: String,
    system_lang: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        core_version: fc_core::VERSION,
        platform: if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        },
        effect: *state.effect.lock().unwrap(),
        lang: state.lang(),
        system_lang: fc_core::shell::system_lang(),
    }
}

/// Hide the window to the tray (or quit if nothing is running).
#[tauri::command]
pub fn close_window(app: AppHandle, state: State<'_, AppState>) {
    crate::close_or_hide(&app, state.active_jobs());
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
