// Release builds on Windows must not open a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod commands;
mod settings;
mod state;
mod texts;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use crate::cli::Aggregator;
use crate::state::AppState;

const TRAY_ID: &str = "main-tray";

pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit("window-visibility", true);
    }
}

/// Close button: hide to the tray while jobs are pending/running, otherwise quit.
pub fn close_or_hide(app: &AppHandle, active_jobs: usize) {
    if active_jobs > 0 {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
        let _ = app.emit("window-visibility", false);
    } else {
        app.exit(0);
    }
}

pub fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

pub fn notify_finished(app: &AppHandle, report: &fc_core::Report) {
    let state = app.state::<AppState>();
    if !state.settings.lock().unwrap().notify {
        return;
    }
    let (title, body) = texts::job_finished(&state.lang(), report);
    notify(app, &title, &body);
}

fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let t = texts::texts(&app.state::<AppState>().lang());
    let open = MenuItem::with_id(app, "open", t.open, true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", t.quit, true, None::<&str>)?;
    Menu::with_items(app, &[&open, &sep, &quit])
}

pub fn rebuild_tray_menu(app: &AppHandle) {
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY_ID), tray_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let mut b = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(texts::texts(&app.state::<AppState>().lang()).tooltip)
        .menu(&tray_menu(app)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    #[cfg(target_os = "macos")]
    {
        b = b.icon_as_template(false);
    }
    b.build(app)?;
    Ok(())
}

/// Mica (Windows 11) / vibrancy (macOS) behind the transparent web view. The UI falls
/// back to an opaque background when this returns "none".
fn apply_window_effect(window: &tauri::WebviewWindow) -> &'static str {
    #[cfg(windows)]
    {
        if window_vibrancy::apply_mica(window, Some(true)).is_ok() {
            return "mica";
        }
    }
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState};
        if window_vibrancy::apply_vibrancy(window, NSVisualEffectMaterial::UnderWindowBackground, Some(NSVisualEffectState::Active), None).is_ok() {
            return "vibrancy";
        }
    }
    let _ = window;
    "none"
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();

    // Installer hooks: register / unregister the shell integration, then exit.
    if argv.iter().any(|a| a == "--register-shell" || a == "--unregister-shell") {
        let code = if argv.iter().any(|a| a == "--register-shell") {
            let exe = std::env::current_exe().unwrap_or_default();
            let lang = settings::Settings::load().resolved_lang();
            #[cfg(windows)]
            if let Some(dir) = exe.parent() {
                if dir.join("fcopy.exe").exists() {
                    let _ = fc_core::shell::add_to_user_path(dir);
                }
            }
            fc_core::shell::install(&exe, &lang)
        } else {
            #[cfg(windows)]
            if let Some(dir) = std::env::current_exe().ok().as_deref().and_then(|e| e.parent()) {
                let _ = fc_core::shell::remove_from_user_path(dir);
            }
            fc_core::shell::uninstall()
        };
        std::process::exit(if code.is_ok() { 0 } else { 1 });
    }

    let app = tauri::Builder::default()
        // Must be first: later launches forward their argv here and exit immediately.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let cwd = std::path::PathBuf::from(cwd);
            match cli::parse(&argv, Some(&cwd)) {
                Some(req) => app.state::<Aggregator>().submit(app, req),
                None => show_main_window(app),
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Aggregator::default())
        .setup(move |app| {
            let handle = app.handle().clone();
            app.manage(AppState::new(&handle));
            if let Some(w) = app.get_webview_window("main") {
                let effect = apply_window_effect(&w);
                *app.state::<AppState>().effect.lock().unwrap() = effect;
            }
            // First run / MSI install: register the context menus unless the user opted out.
            // Release builds only, so dev runs never point Explorer at a target/debug exe.
            if cfg!(not(debug_assertions)) {
                let (auto, lang) = {
                    let st = app.state::<AppState>();
                    let auto = st.settings.lock().unwrap().auto_shell;
                    (auto, st.lang())
                };
                if auto {
                    std::thread::spawn(move || {
                        if !fc_core::shell::is_installed() {
                            if let Ok(exe) = std::env::current_exe() {
                                let _ = fc_core::shell::install(&exe, &lang);
                            }
                        }
                    });
                }
            }
            if let Err(e) = build_tray(&handle) {
                eprintln!("tray icon unavailable: {e}");
            }
            show_main_window(&handle);
            if let Some(req) = cli::parse(&argv, None) {
                app.state::<Aggregator>().submit(&handle, req);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let app = window.app_handle();
                    close_or_hide(app, app.state::<AppState>().active_jobs());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_volumes,
            commands::volume_info,
            commands::plan_job,
            commands::start_plan,
            commands::pause_job,
            commands::resume_job,
            commands::cancel_job,
            commands::reorder_job,
            commands::clear_finished,
            commands::list_queue,
            commands::resolve_conflict,
            commands::resolve_disk_full,
            commands::pending_requests,
            commands::format_drive,
            commands::eject_volume,
            commands::shell_status,
            commands::shell_install,
            commands::shell_uninstall,
            commands::paste_source,
            commands::list_jobs,
            commands::resume_stored_job,
            commands::discard_stored_job,
            commands::get_settings,
            commands::set_settings,
            commands::system_lang,
            commands::take_intents,
            commands::app_info,
            commands::close_window,
            commands::quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Fast Copy");

    app.run(|app, event| match event {
        // Files dropped on the Dock icon / "Open with Fast Copy" in Finder.
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => {
            let paths: Vec<std::path::PathBuf> = urls.iter().filter_map(|u| u.to_file_path().ok()).collect();
            if !paths.is_empty() {
                app.state::<Aggregator>().submit(app, cli::Request { verb: cli::Verb::SendTo, paths });
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main_window(app),
        RunEvent::ExitRequested { api, code, .. } => {
            // Closing the last window must not quit while jobs run (hidden to tray);
            // an explicit app.exit(…) carries a code and is honoured.
            if code.is_none() && app.state::<AppState>().active_jobs() > 0 {
                api.prevent_exit();
            }
        }
        _ => {}
    });
}
