//! Copying locked / in-use files through a Volume Shadow Copy snapshot (Windows, admin).
//!
//! The unelevated app writes a list of `(source, destination)` pairs to a JSON file and
//! starts the elevated `fc-helper vss-copy`, which creates one snapshot per source
//! volume (WMI `Win32_ShadowCopy.Create`), copies each file from the snapshot with the
//! normal verified engine, deletes the snapshots and writes a JSON result file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VssPair {
    pub src: PathBuf,
    pub dst: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VssResult {
    pub src: PathBuf,
    pub ok: bool,
    pub error: Option<String>,
    pub bytes: u64,
}

/// Run the elevated helper on `pairs`. Returns one result per pair.
#[cfg(windows)]
pub fn copy_locked(pairs: &[VssPair], verify: crate::types::VerifyMode) -> Result<Vec<VssResult>> {
    let dir = std::env::temp_dir();
    let id = uuid::Uuid::new_v4();
    let input = dir.join(format!("fastcopy-vss-{id}.json"));
    let output = dir.join(format!("fastcopy-vss-{id}.out.json"));
    std::fs::write(&input, serde_json::to_vec(pairs).unwrap()).map_err(|e| Error::io("écriture", &input, e))?;
    let v = match verify {
        crate::types::VerifyMode::Fast => "fast",
        crate::types::VerifyMode::Full => "full",
        crate::types::VerifyMode::Paranoid => "paranoid",
    };
    let args = format!("vss-copy --input \"{}\" --output \"{}\" --verify {v}", input.display(), output.display());
    let code = crate::format::run_elevated(&crate::format::helper_path(), &args)?;
    let _ = std::fs::remove_file(&input);
    if code == 1223 {
        return Err(Error::Other("élévation refusée : fichiers verrouillés non copiés".into()));
    }
    let data = std::fs::read(&output).map_err(|e| Error::Other(format!("résultat VSS absent (code {code}) : {e}")))?;
    let _ = std::fs::remove_file(&output);
    serde_json::from_slice(&data).map_err(|e| Error::Other(format!("résultat VSS invalide : {e}")))
}

#[cfg(not(windows))]
pub fn copy_locked(_pairs: &[VssPair], _verify: crate::types::VerifyMode) -> Result<Vec<VssResult>> {
    Err(Error::Other("VSS n'existe que sous Windows".into()))
}

/// Inside the elevated helper: create a snapshot of `volume` (e.g. `C:\`).
/// Returns (shadow id, device object path).
#[cfg(windows)]
pub fn create_snapshot(volume: &str) -> Result<(String, String)> {
    let ps = format!(
        "$ErrorActionPreference='Stop';\
         $r = Invoke-CimMethod -ClassName Win32_ShadowCopy -MethodName Create -Arguments @{{Volume='{volume}';Context='ClientAccessible'}};\
         if ($r.ReturnValue -ne 0) {{ throw \"Win32_ShadowCopy.Create code $($r.ReturnValue)\" }};\
         $s = Get-CimInstance Win32_ShadowCopy | Where-Object {{ $_.ID -eq $r.ShadowID }};\
         Write-Output \"$($r.ShadowID)|$($s.DeviceObject)\""
    );
    let out = powershell(&ps)?;
    let line = out.lines().rev().find(|l| l.contains('|')).unwrap_or("").trim().to_string();
    let (id, dev) = line.split_once('|').ok_or_else(|| Error::Other(format!("création du cliché VSS échouée : {out}")))?;
    Ok((id.to_string(), dev.to_string()))
}

#[cfg(windows)]
pub fn delete_snapshot(id: &str) -> Result<()> {
    let ps = format!("Get-CimInstance Win32_ShadowCopy | Where-Object {{ $_.ID -eq '{id}' }} | Remove-CimInstance");
    powershell(&ps).map(|_| ())
}

#[cfg(windows)]
fn powershell(script: &str) -> Result<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| Error::Other(format!("powershell : {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        return Err(Error::Other(format!("{text}{}", String::from_utf8_lossy(&out.stderr))));
    }
    Ok(text)
}
