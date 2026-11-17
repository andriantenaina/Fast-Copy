//! Reformatting a USB drive to exFAT (cross-platform: read/write on Windows and macOS,
//! no 4 GiB limit). Destructive: callers must obtain explicit user confirmation.
//!
//! Parameters: MBR partition scheme, 1 MiB alignment, cluster ≤ 1 MiB (macOS refuses
//! to mount exFAT with ≥ 2 MiB clusters).
//!
//! * Windows: an elevated `fc-helper` re-checks the target and runs a diskpart script.
//! * macOS: `diskutil eraseDisk ExFAT <label> MBRFormat /dev/diskN` (no sudo for external disks).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::volume::{self, VolumeInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatRequest {
    /// Any path on the volume to format (e.g. `E:\` or `/Volumes/KEY`).
    pub volume: PathBuf,
    pub label: String,
    /// Cluster size in bytes (Windows only; macOS uses diskutil's default).
    pub cluster: Option<u64>,
}

/// Refuse anything that is not clearly an external, non-system drive.
pub fn check_formattable(v: &VolumeInfo) -> Result<()> {
    if v.is_system {
        return Err(Error::Other("refus : c'est le disque système".into()));
    }
    if v.is_network() {
        return Err(Error::Other("refus : lecteur réseau".into()));
    }
    if !v.is_external() {
        return Err(Error::Other(format!(
            "refus : « {} » n'est pas détecté comme un support amovible ou USB (bus : {:?})",
            v.root.display(),
            v.bus
        )));
    }
    if v.disk_id.is_none() {
        return Err(Error::Other("disque physique introuvable".into()));
    }
    Ok(())
}

/// exFAT labels are limited to 11 characters; strip characters diskpart/diskutil dislike.
pub fn clean_label(label: &str) -> String {
    let s: String = label
        .chars()
        .filter(|c| !matches!(c, '"' | '\'' | '\\' | '/' | ':' | '*' | '?' | '<' | '>' | '|' | '=' | ';' | ',') && !c.is_control())
        .take(11)
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "FASTCOPY".into()
    } else {
        s
    }
}

pub fn valid_cluster(c: u64) -> bool {
    c.is_power_of_two() && (512..=1024 * 1024).contains(&c)
}

/// Path of the elevated helper shipped next to the current executable.
pub fn helper_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let name = if cfg!(windows) { "fc-helper.exe" } else { "fc-helper" };
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let direct = dir.join(name);
    if direct.exists() {
        return direct;
    }
    // Tauri may place resources/sidecars with a target-triple suffix.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("fc-helper") && (!cfg!(windows) || n.ends_with(".exe")) {
                return e.path();
            }
        }
    }
    direct
}

/// Format the volume to exFAT. Blocks until done.
pub fn format_exfat(req: &FormatRequest) -> Result<VolumeInfo> {
    let v = volume::volume_info(&req.volume).map_err(|e| Error::Other(format!("volume introuvable : {e}")))?;
    check_formattable(&v)?;
    let label = clean_label(&req.label);
    let cluster = req.cluster.unwrap_or_else(|| volume::recommended_exfat_cluster(v.total_bytes, false));
    if !valid_cluster(cluster) {
        return Err(Error::Other("taille de cluster invalide (512 o à 1 Mo, puissance de 2)".into()));
    }
    imp::format(&v, &label, cluster)?;
    // Wait for the new volume to mount, then limit OS clutter on it.
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Some(nv) = find_formatted(&v, &label) {
            prepare_fresh_volume(&nv.root);
            return Ok(nv);
        }
    }
    Err(Error::Other("formatage terminé mais le volume n'est pas réapparu ; débranchez et rebranchez la clé".into()))
}

fn find_formatted(old: &VolumeInfo, label: &str) -> Option<VolumeInfo> {
    volume::list_volumes().into_iter().find(|n| n.fs == volume::FsKind::ExFat && (n.disk_id == old.disk_id || n.label == label))
}

/// Ask macOS not to index / log on the freshly formatted drive (harmless on Windows).
pub fn prepare_fresh_volume(root: &Path) {
    let _ = std::fs::write(root.join(".metadata_never_index"), b"");
    let fse = root.join(".fseventsd");
    if std::fs::create_dir_all(&fse).is_ok() {
        let _ = std::fs::write(fse.join("no_log"), b"");
    }
    #[cfg(windows)]
    {
        let _ = crate::sys::set_attributes(&root.join(".metadata_never_index"), 0x2);
        let _ = crate::sys::set_attributes(&fse, 0x2 | 0x10);
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::sys::wide;
    use std::ffi::OsStr;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

    pub fn format(v: &VolumeInfo, label: &str, cluster: u64) -> Result<()> {
        let disk = v.disk_id.clone().unwrap_or_default();
        let letter = v.root.to_string_lossy().chars().next().unwrap_or('?');
        let args = format!(
            "format --disk {disk} --letter {letter} --expect-size {} --label \"{label}\" --cluster {cluster}",
            v.total_bytes
        );
        let code = run_elevated(&helper_path(), &args)?;
        match code {
            0 => Ok(()),
            1223 => Err(Error::Other("élévation refusée par l'utilisateur".into())),
            c => Err(Error::Other(format!("échec du formatage (code {c}) ; voir le journal de fc-helper"))),
        }
    }

    /// Run `exe args` elevated (UAC prompt) and wait for its exit code.
    pub fn run_elevated(exe: &Path, args: &str) -> Result<u32> {
        if !exe.exists() {
            return Err(Error::Other(format!("assistant introuvable : {}", exe.display())));
        }
        let verb = wide(OsStr::new("runas"));
        let file = wide(exe.as_os_str());
        let params = wide(OsStr::new(args));
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0; // SW_HIDE
        // SAFETY: struct initialised; strings outlive the call.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(1223) {
                return Ok(1223);
            }
            return Err(Error::Other(format!("lancement élevé impossible : {e}")));
        }
        let mut code = 1u32;
        // SAFETY: valid process handle.
        unsafe {
            WaitForSingleObject(info.hProcess, INFINITE);
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
        }
        Ok(code)
    }

    /// Executed *inside* the elevated helper: re-validate, then diskpart.
    pub fn diskpart_format(disk: u32, letter: char, expect_size: u64, label: &str, cluster: u64) -> Result<String> {
        let root = PathBuf::from(format!("{letter}:\\"));
        let v = volume::volume_info(&root).map_err(|e| Error::Other(format!("volume {letter}: introuvable : {e}")))?;
        check_formattable(&v)?;
        if v.disk_id.as_deref() != Some(&disk.to_string()) {
            return Err(Error::Other(format!("le lecteur {letter}: n'est plus sur le disque {disk} : abandon")));
        }
        if v.total_bytes != expect_size {
            return Err(Error::Other("la taille du volume a changé depuis la confirmation : abandon".into()));
        }
        let unit = if cluster >= 1024 * 1024 { format!("{}M", cluster / (1024 * 1024)) } else { format!("{}K", cluster / 1024) };
        let script = format!(
            "select disk {disk}\r\nclean\r\nconvert mbr\r\ncreate partition primary align=1024\r\nformat fs=exfat unit={unit} quick label=\"{}\"\r\nassign letter={letter} noerr\r\nexit\r\n",
            clean_label(label)
        );
        let path = std::env::temp_dir().join(format!("fastcopy-diskpart-{}.txt", std::process::id()));
        std::fs::write(&path, script).map_err(|e| Error::io("écriture du script", &path, e))?;
        let out = std::process::Command::new("diskpart.exe").arg("/s").arg(&path).output().map_err(|e| Error::io("diskpart", &path, e))?;
        let _ = std::fs::remove_file(&path);
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if !out.status.success() {
            return Err(Error::Other(format!("diskpart a échoué : {text}")));
        }
        Ok(text)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    pub fn format(v: &VolumeInfo, label: &str, _cluster: u64) -> Result<()> {
        let disk = v.disk_id.clone().ok_or_else(|| Error::Other("disque introuvable".into()))?;
        let out = std::process::Command::new("/usr/sbin/diskutil")
            .args(["eraseDisk", "ExFAT", label, "MBRFormat", &format!("/dev/{disk}")])
            .output()
            .map_err(|e| Error::io("diskutil", Path::new("/usr/sbin/diskutil"), e))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(Error::Other(format!("diskutil : {}", String::from_utf8_lossy(&out.stderr).trim())))
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    pub fn format(_v: &VolumeInfo, _label: &str, _cluster: u64) -> Result<()> {
        Err(Error::Other("formatage non pris en charge sur ce système".into()))
    }
}

#[cfg(windows)]
pub use imp::{diskpart_format, run_elevated};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels() {
        assert_eq!(clean_label("Ma Clé USB Super Longue"), "Ma Clé USB");
        assert_eq!(clean_label("\"\""), "FASTCOPY");
        assert!(valid_cluster(128 * 1024));
        assert!(!valid_cluster(2 * 1024 * 1024));
    }
}
