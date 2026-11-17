//! Operating-system integration: right-click menus, "Send To", Quick Actions, and the
//! "copy with Fast Copy → paste here with Fast Copy" clipboard.
//!
//! * Windows: per-user registry verbs (`HKCU\Software\Classes`, no admin), shown in the
//!   Windows 10 menu and under "Show more options" on Windows 11 (the Windows 11 top-level
//!   menu is provided by the `fc-shellext-win` COM DLL + sparse package), plus a Send To
//!   shortcut. "Paste here" also understands files copied with Ctrl+C in Explorer.
//! * macOS: Automator Quick Actions in `~/Library/Services` (Finder right-click menu).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::Mode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppClipboard {
    pub mode: Mode,
    pub paths: Vec<PathBuf>,
    pub time_ms: i64,
    /// Windows clipboard sequence number when this was set (to know which is newer).
    #[serde(default)]
    pub sys_seq: u32,
}

fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("FastCopy")
}

pub fn clipboard_path() -> PathBuf {
    data_dir().join("clipboard.json")
}

pub fn set_app_clipboard(mode: Mode, paths: Vec<PathBuf>) -> Result<()> {
    let c = AppClipboard {
        mode,
        paths,
        time_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
        sys_seq: system_clipboard_seq(),
    };
    std::fs::create_dir_all(data_dir()).map_err(|e| Error::io("création", data_dir(), e))?;
    std::fs::write(clipboard_path(), serde_json::to_vec(&c).unwrap())
        .map_err(|e| Error::io("écriture", clipboard_path(), e))
}

pub fn get_app_clipboard() -> Option<AppClipboard> {
    serde_json::from_slice(&std::fs::read(clipboard_path()).ok()?).ok()
}

pub fn clear_app_clipboard() {
    let _ = std::fs::remove_file(clipboard_path());
}

/// Files to paste: the most recent of Fast Copy's own clipboard and the system
/// clipboard (files copied/cut with Ctrl+C / Ctrl+X in Explorer).
pub fn paste_source() -> Option<(Vec<PathBuf>, Mode)> {
    let app = get_app_clipboard().filter(|c| !c.paths.is_empty());
    let sys = system_clipboard_files();
    match (app, sys) {
        (Some(a), Some(s)) => {
            if system_clipboard_seq() != a.sys_seq {
                Some(s)
            } else {
                Some((a.paths, a.mode))
            }
        }
        (Some(a), None) => Some((a.paths, a.mode)),
        (None, s) => s,
    }
}

#[cfg(windows)]
pub fn system_clipboard_seq() -> u32 {
    // SAFETY: no arguments.
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

#[cfg(not(windows))]
pub fn system_clipboard_seq() -> u32 {
    0
}

/// Files on the Windows clipboard (CF_HDROP) and whether they were cut (move).
#[cfg(windows)]
pub fn system_clipboard_files() -> Option<(Vec<PathBuf>, Mode)> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DragQueryFileW;
    // SAFETY: standard clipboard protocol; handles are not freed (owned by the clipboard).
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP as u32) == 0 {
            return None;
        }
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened {
            return None;
        }
        let mut files = Vec::new();
        let h = GetClipboardData(CF_HDROP as u32);
        if !h.is_null() {
            let count = DragQueryFileW(h as _, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
            for i in 0..count {
                let len = DragQueryFileW(h as _, i, std::ptr::null_mut(), 0);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(h as _, i, buf.as_mut_ptr(), buf.len() as u32);
                buf.truncate(len as usize);
                files.push(PathBuf::from(std::ffi::OsString::from_wide(&buf)));
            }
        }
        let mut mode = Mode::Copy;
        let fmt_name: Vec<u16> = "Preferred DropEffect"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let fmt = RegisterClipboardFormatW(fmt_name.as_ptr());
        if fmt != 0 {
            let hd = GetClipboardData(fmt);
            if !hd.is_null() {
                let p = GlobalLock(hd as _) as *const u32;
                if !p.is_null() {
                    if *p & 2 != 0 {
                        mode = Mode::Move; // DROPEFFECT_MOVE (cut)
                    }
                    GlobalUnlock(hd as _);
                }
            }
        }
        CloseClipboard();
        if files.is_empty() {
            None
        } else {
            Some((files, mode))
        }
    }
}

#[cfg(not(windows))]
pub fn system_clipboard_files() -> Option<(Vec<PathBuf>, Mode)> {
    None
}

/// Menu labels.
pub struct Labels {
    pub copy: &'static str,
    pub moves: &'static str,
    pub paste: &'static str,
    pub send_to: &'static str,
}

pub fn labels(lang: &str) -> Labels {
    if lang.starts_with("fr") {
        Labels {
            copy: "Copier avec Fast Copy",
            moves: "Déplacer avec Fast Copy",
            paste: "Coller ici avec Fast Copy",
            send_to: "Fast Copy",
        }
    } else {
        Labels {
            copy: "Copy with Fast Copy",
            moves: "Move with Fast Copy",
            paste: "Paste here with Fast Copy",
            send_to: "Fast Copy",
        }
    }
}

/// System UI language ("fr", "en"…).
pub fn system_lang() -> String {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() && v != "C" && v != "POSIX" {
                return v[..2.min(v.len())].to_lowercase();
            }
        }
    }
    #[cfg(windows)]
    {
        // GetUserDefaultUILanguage: primary language id 0x0C = French.
        extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        // SAFETY: no arguments.
        let lang = unsafe { GetUserDefaultUILanguage() };
        if lang & 0x3ff == 0x0c {
            return "fr".into();
        }
        return "en".into();
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(first) = s.split('"').nth(1) {
                return first[..2.min(first.len())].to_lowercase();
            }
        }
    }
    #[allow(unreachable_code)]
    "en".into()
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::sys::wide;
    use std::ffi::OsStr;
    use windows_sys::Win32::System::Registry::*;
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED};

    const VERBS: &[(&str, &str)] = &[
        (r"Software\Classes\*\shell\FastCopy.Copy", "copy"),
        (r"Software\Classes\*\shell\FastCopy.Move", "move"),
        (r"Software\Classes\Directory\shell\FastCopy.Copy", "copy"),
        (r"Software\Classes\Directory\shell\FastCopy.Move", "move"),
        (r"Software\Classes\Directory\shell\FastCopy.Paste", "paste"),
        (
            r"Software\Classes\Directory\Background\shell\FastCopy.Paste",
            "paste-bg",
        ),
        (r"Software\Classes\Drive\shell\FastCopy.Paste", "paste"),
    ];

    fn set(key: &str, name: Option<&str>, value: &str) -> Result<()> {
        let k = wide(OsStr::new(key));
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: valid strings; key closed below.
        let r = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null(),
                &mut h,
                std::ptr::null_mut(),
            )
        };
        if r != 0 {
            return Err(Error::Other(format!(
                "registre : création de {key} (code {r})"
            )));
        }
        let n = name.map(|n| wide(OsStr::new(n)));
        let v = wide(OsStr::new(value));
        let r = unsafe {
            RegSetValueExW(
                h,
                n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                0,
                REG_SZ,
                v.as_ptr() as *const u8,
                (v.len() * 2) as u32,
            )
        };
        unsafe { RegCloseKey(h) };
        if r != 0 {
            return Err(Error::Other(format!(
                "registre : écriture dans {key} (code {r})"
            )));
        }
        Ok(())
    }

    fn exists(key: &str) -> bool {
        let k = wide(OsStr::new(key));
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: valid args.
        let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_READ, &mut h) };
        if r == 0 {
            unsafe { RegCloseKey(h) };
            true
        } else {
            false
        }
    }

    fn send_to_link() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_default()
            .join(r"Microsoft\Windows\SendTo\Fast Copy.lnk")
    }

    pub fn install(app: &Path, lang: &str) -> Result<()> {
        let l = labels(lang);
        let exe = app.display().to_string();
        for (key, verb) in VERBS {
            let (label, arg) = match *verb {
                "copy" => (l.copy, "\"%1\""),
                "move" => (l.moves, "\"%1\""),
                "paste" => (l.paste, "\"%1\""),
                _ => (l.paste, "\"%V\""),
            };
            let cmd_verb = if verb.starts_with("paste") {
                "paste"
            } else {
                verb
            };
            set(key, None, label)?;
            set(key, Some("MUIVerb"), label)?;
            set(key, Some("Icon"), &format!("\"{exe}\",0"))?;
            set(key, Some("MultiSelectModel"), "Player")?;
            set(
                &format!(r"{key}\command"),
                None,
                &format!("\"{exe}\" --shell {cmd_verb} {arg}"),
            )?;
        }
        // Send To shortcut through the WScript.Shell COM object.
        let link = send_to_link();
        let ps = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='--shell sendto';$s.IconLocation='{},0';$s.Save()",
            link.display().to_string().replace('\'', "''"),
            exe.replace('\'', "''"),
            exe.replace('\'', "''")
        );
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .creation_flags(0x0800_0000)
            .status();
        refresh();
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        for (key, _) in VERBS {
            let k = wide(OsStr::new(key));
            // SAFETY: valid args.
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
        }
        let _ = std::fs::remove_file(send_to_link());
        refresh();
        Ok(())
    }

    pub fn is_installed() -> bool {
        exists(VERBS[0].0)
    }

    fn refresh() {
        // SAFETY: notification without items.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    fn services_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join("Library/Services")
    }

    fn workflows(lang: &str) -> Vec<(String, &'static str, bool)> {
        let l = labels(lang);
        vec![
            (l.copy.to_string(), "copy", false),
            (l.moves.to_string(), "move", false),
            (l.paste.to_string(), "paste", true),
        ]
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn info_plist(label: &str, folders_only: bool) -> String {
        let types = if folders_only {
            "<string>public.folder</string>"
        } else {
            "<string>public.item</string>"
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSBackgroundColorName</key><string>background</string>
			<key>NSIconName</key><string>NSActionTemplate</string>
			<key>NSMenuItem</key><dict><key>default</key><string>{label}</string></dict>
			<key>NSMessage</key><string>runWorkflowAsService</string>
			<key>NSRequiredContext</key><dict><key>NSApplicationIdentifier</key><string>com.apple.finder</string></dict>
			<key>NSSendFileTypes</key><array>{types}</array>
		</dict>
	</array>
</dict>
</plist>
"#,
            label = xml_escape(label)
        )
    }

    fn document_wflow(command: &str, folders_only: bool) -> String {
        let input = if folders_only {
            "com.apple.Automator.folder"
        } else {
            "com.apple.Automator.fileSystemObject"
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key><string>521</string>
	<key>AMApplicationVersion</key><string>2.10</string>
	<key>AMDocumentVersion</key><string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key><string>List</string>
					<key>Optional</key><true/>
					<key>Types</key><array><string>com.apple.cocoa.string</string></array>
				</dict>
				<key>AMActionVersion</key><string>2.0.3</string>
				<key>AMApplication</key><array><string>Automator</string></array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key><dict/>
					<key>CheckedForUserDefaultShell</key><dict/>
					<key>inputMethod</key><dict/>
					<key>shell</key><dict/>
					<key>source</key><dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key><string>List</string>
					<key>Types</key><array><string>com.apple.cocoa.string</string></array>
				</dict>
				<key>ActionBundlePath</key><string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key><string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key><string>{cmd}</string>
					<key>CheckedForUserDefaultShell</key><true/>
					<key>inputMethod</key><integer>1</integer>
					<key>shell</key><string>/bin/sh</string>
					<key>source</key><string></string>
				</dict>
				<key>BundleIdentifier</key><string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key><string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key><false/>
				<key>CanShowWhenRun</key><true/>
				<key>Category</key><array><string>AMCategoryUtilities</string></array>
				<key>Class Name</key><string>RunShellScriptAction</string>
				<key>InputUUID</key><string>A1B2C3D4-0000-4000-8000-000000000001</string>
				<key>Keywords</key><array><string>Shell</string></array>
				<key>OutputUUID</key><string>A1B2C3D4-0000-4000-8000-000000000002</string>
				<key>UUID</key><string>A1B2C3D4-0000-4000-8000-000000000003</string>
				<key>UnlocalizedApplications</key><array><string>Automator</string></array>
				<key>arguments</key><dict/>
				<key>isViewVisible</key><integer>1</integer>
				<key>location</key><string>309.000000:253.000000</string>
			</dict>
			<key>isViewVisible</key><integer>1</integer>
		</dict>
	</array>
	<key>connectors</key><dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleIDsByPath</key><dict/>
		<key>applicationPaths</key><array/>
		<key>inputTypeIdentifier</key><string>{input}</string>
		<key>outputTypeIdentifier</key><string>com.apple.Automator.nothing</string>
		<key>presentationMode</key><integer>15</integer>
		<key>processesInput</key><integer>0</integer>
		<key>serviceApplicationBundleID</key><string>com.apple.finder</string>
		<key>serviceApplicationPath</key><string>/System/Library/CoreServices/Finder.app</string>
		<key>serviceInputTypeIdentifier</key><string>{input}</string>
		<key>serviceOutputTypeIdentifier</key><string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key><integer>0</integer>
		<key>systemImageName</key><string>NSActionTemplate</string>
		<key>useAutomaticInputType</key><integer>0</integer>
		<key>workflowTypeIdentifier</key><string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#,
            cmd = xml_escape(command)
        )
    }

    fn bundle_name(label: &str) -> String {
        format!("{}.workflow", label.replace('/', "-"))
    }

    pub fn install(app: &Path, lang: &str) -> Result<()> {
        let dir = services_dir();
        let exe = app.display().to_string().replace('"', "\\\"");
        for (label, verb, folders) in workflows(lang) {
            let wf = dir.join(bundle_name(&label)).join("Contents");
            std::fs::create_dir_all(&wf).map_err(|e| Error::io("création", &wf, e))?;
            let cmd = format!("\"{exe}\" --shell {verb} \"$@\" >/dev/null 2>&1 &");
            std::fs::write(wf.join("Info.plist"), info_plist(&label, folders))
                .map_err(|e| Error::io("écriture", &wf, e))?;
            std::fs::write(wf.join("document.wflow"), document_wflow(&cmd, folders))
                .map_err(|e| Error::io("écriture", &wf, e))?;
        }
        let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
            .arg("-update")
            .status();
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        for lang in ["fr", "en"] {
            for (label, _, _) in workflows(lang) {
                let _ = std::fs::remove_dir_all(services_dir().join(bundle_name(&label)));
            }
        }
        let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
            .arg("-update")
            .status();
        Ok(())
    }

    pub fn is_installed() -> bool {
        ["fr", "en"]
            .iter()
            .any(|l| services_dir().join(bundle_name(labels(l).copy)).exists())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    pub fn install(_app: &Path, _lang: &str) -> Result<()> {
        Err(Error::Other(
            "intégration non prise en charge sur ce système".into(),
        ))
    }
    pub fn uninstall() -> Result<()> {
        Ok(())
    }
    pub fn is_installed() -> bool {
        false
    }
}

/// Add `dir` (where `fcopy` lives) to the user's PATH. Windows: `HKCU\Environment\Path`
/// (no admin), then broadcast the change. Returns true if PATH was modified.
#[cfg(windows)]
pub fn add_to_user_path(dir: &Path) -> Result<bool> {
    let cur = path_env::read();
    let d = dir.display().to_string();
    if cur.split(';').any(|p| {
        p.trim_end_matches('\\')
            .eq_ignore_ascii_case(d.trim_end_matches('\\'))
    }) {
        return Ok(false);
    }
    let new = if cur.trim().is_empty() {
        d
    } else {
        format!("{};{d}", cur.trim_end_matches(';'))
    };
    path_env::write(&new)?;
    Ok(true)
}

/// Remove `dir` from the user's PATH. Returns true if PATH was modified.
#[cfg(windows)]
pub fn remove_from_user_path(dir: &Path) -> Result<bool> {
    let cur = path_env::read();
    let d = dir.display().to_string();
    let kept: Vec<&str> = cur
        .split(';')
        .filter(|p| {
            !p.is_empty()
                && !p
                    .trim_end_matches('\\')
                    .eq_ignore_ascii_case(d.trim_end_matches('\\'))
        })
        .collect();
    let new = kept.join(";");
    if new == cur.trim_end_matches(';') {
        return Ok(false);
    }
    path_env::write(&new)?;
    Ok(true)
}

#[cfg(windows)]
mod path_env {
    use super::*;
    use crate::sys::wide;
    use std::ffi::OsStr;
    use windows_sys::Win32::System::Registry::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    pub fn read() -> String {
        let key = wide(OsStr::new("Environment"));
        let name = wide(OsStr::new("Path"));
        let mut buf = vec![0u16; 32768];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: valid buffers; RRF_NOEXPAND keeps %VARS% unexpanded.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut _,
                &mut size,
            )
        };
        if r != 0 {
            return String::new();
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..len])
    }

    pub fn write(value: &str) -> Result<()> {
        let key = wide(OsStr::new("Environment"));
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: valid args; key closed below.
        let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_WRITE, &mut h) };
        if r != 0 {
            return Err(Error::Other(format!("registre : Environment (code {r})")));
        }
        let name = wide(OsStr::new("Path"));
        let v = wide(OsStr::new(value));
        let r = unsafe {
            RegSetValueExW(
                h,
                name.as_ptr(),
                0,
                REG_EXPAND_SZ,
                v.as_ptr() as *const u8,
                (v.len() * 2) as u32,
            )
        };
        unsafe { RegCloseKey(h) };
        if r != 0 {
            return Err(Error::Other(format!(
                "registre : écriture de Path (code {r})"
            )));
        }
        let env = wide(OsStr::new("Environment"));
        let mut res = 0usize;
        // Tell Explorer / new terminals that the environment changed.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                env.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                2000,
                &mut res,
            )
        };
        Ok(())
    }
}

/// Register the right-click menus for `app` (the GUI executable).
pub fn install(app: &Path, lang: &str) -> Result<()> {
    imp::install(app, lang)
}

pub fn uninstall() -> Result<()> {
    imp::uninstall()
}

pub fn is_installed() -> bool {
    imp::is_installed()
}
