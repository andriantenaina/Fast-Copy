//! `fc-helper` — small helper started *elevated* (UAC) by Fast Copy for the few
//! operations that need administrator rights:
//!
//! * `format --disk N --letter E --expect-size BYTES --label L --cluster BYTES`
//!   Re-validates the target (USB / removable, not the system disk, same disk number
//!   and size as confirmed by the user) then runs diskpart (MBR, 1 MiB alignment, exFAT).
//! * `vss-copy --input pairs.json --output results.json --verify full`
//!   Copies locked files from Volume Shadow Copy snapshots.
//!
//! Every run appends to `%TEMP%\fastcopy-helper.log`.

use std::collections::HashMap;
use std::io::Write;
#[cfg(windows)]
use std::path::{Path, PathBuf};

fn log(msg: &str) {
    let p = std::env::temp_dir().join("fastcopy-helper.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "[{}] {msg}", std::process::id());
    }
}

fn args_map(args: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(k) = args[i].strip_prefix("--") {
            let v = args.get(i + 1).filter(|v| !v.starts_with("--")).cloned().unwrap_or_default();
            if !v.is_empty() {
                i += 1;
            }
            m.insert(k.to_string(), v);
        }
        i += 1;
    }
    m
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("usage: fc-helper format|vss-copy …");
        std::process::exit(64);
    };
    let m = args_map(&args[1..]);
    log(&format!("start {cmd} {m:?}"));
    let code = match cmd.as_str() {
        "format" => format_cmd(&m),
        "vss-copy" => vss_cmd(&m),
        _ => {
            eprintln!("commande inconnue : {cmd}");
            64
        }
    };
    log(&format!("exit {code}"));
    std::process::exit(code);
}

#[cfg(windows)]
fn format_cmd(m: &HashMap<String, String>) -> i32 {
    let disk: u32 = match m.get("disk").and_then(|v| v.parse().ok()) {
        Some(d) => d,
        None => return 64,
    };
    let letter = m.get("letter").and_then(|v| v.chars().next()).unwrap_or('?');
    let expect: u64 = m.get("expect-size").and_then(|v| v.parse().ok()).unwrap_or(0);
    let label = m.get("label").cloned().unwrap_or_else(|| "FASTCOPY".into());
    let cluster: u64 = m.get("cluster").and_then(|v| v.parse().ok()).unwrap_or(128 * 1024);
    match fc_core::format::diskpart_format(disk, letter, expect, &label, cluster) {
        Ok(out) => {
            log(&format!("diskpart ok:\n{out}"));
            0
        }
        Err(e) => {
            log(&format!("format error: {e}"));
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(not(windows))]
fn format_cmd(_m: &HashMap<String, String>) -> i32 {
    eprintln!("format : Windows uniquement (macOS utilise diskutil sans assistant)");
    64
}

#[cfg(windows)]
fn vss_cmd(m: &HashMap<String, String>) -> i32 {
    use fc_core::vss::{create_snapshot, delete_snapshot, VssPair, VssResult};
    let (Some(input), Some(output)) = (m.get("input"), m.get("output")) else { return 64 };
    let verify = match m.get("verify").map(String::as_str) {
        Some("fast") => fc_core::VerifyMode::Fast,
        Some("paranoid") => fc_core::VerifyMode::Paranoid,
        _ => fc_core::VerifyMode::Full,
    };
    let pairs: Vec<VssPair> = match std::fs::read(input).ok().and_then(|d| serde_json::from_slice(&d).ok()) {
        Some(p) => p,
        None => return 65,
    };
    // One snapshot per source volume.
    let mut snapshots: HashMap<PathBuf, Result<(String, String), String>> = HashMap::new();
    let mut results = Vec::with_capacity(pairs.len());
    for p in &pairs {
        let root = match fc_core::volume::volume_root(&p.src) {
            Ok(r) => r,
            Err(e) => {
                results.push(VssResult { src: p.src.clone(), ok: false, error: Some(e.to_string()), bytes: 0 });
                continue;
            }
        };
        let snap = snapshots
            .entry(root.clone())
            .or_insert_with(|| create_snapshot(&root.display().to_string()).map_err(|e| e.to_string()))
            .clone();
        let r = match snap {
            Err(e) => VssResult { src: p.src.clone(), ok: false, error: Some(e), bytes: 0 },
            Ok((_, device)) => {
                let rel = p.src.strip_prefix(&root).unwrap_or(Path::new(""));
                let snap_src = PathBuf::from(format!("{}\\{}", device.trim_end_matches('\\'), rel.display()));
                match fc_core::job::copy_one(&snap_src, &p.dst, verify) {
                    Ok(bytes) => VssResult { src: p.src.clone(), ok: true, error: None, bytes },
                    Err(e) => VssResult { src: p.src.clone(), ok: false, error: Some(e.to_string()), bytes: 0 },
                }
            }
        };
        log(&format!("{} -> ok={} {:?}", p.src.display(), r.ok, r.error));
        results.push(r);
    }
    for (_, s) in snapshots {
        if let Ok((id, _)) = s {
            if let Err(e) = delete_snapshot(&id) {
                log(&format!("delete snapshot {id}: {e}"));
            }
        }
    }
    match std::fs::write(output, serde_json::to_vec(&results).unwrap_or_default()) {
        Ok(()) => 0,
        Err(_) => 74,
    }
}

#[cfg(not(windows))]
fn vss_cmd(_m: &HashMap<String, String>) -> i32 {
    eprintln!("vss-copy : Windows uniquement");
    64
}
