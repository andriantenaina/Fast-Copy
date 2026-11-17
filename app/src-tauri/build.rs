use std::path::{Path, PathBuf};

/// `tauri-build` refuses to run when an `externalBin` or a bundle resource is missing.
/// Debug builds (plain `cargo build`) get empty placeholders so the crate always compiles.
/// Release builds fail loudly instead, so an empty helper/DLL/MSIX can never be shipped —
/// unless `FASTCOPY_ALLOW_PLACEHOLDERS=1` is set (local experiments only).
fn ensure(path: &Path, what: &str, how: &str) {
    let ok = std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);
    println!("cargo:rerun-if-changed={}", path.display());
    if ok {
        return;
    }
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let allow = std::env::var("FASTCOPY_ALLOW_PLACEHOLDERS").as_deref() == Ok("1");
    if release && !allow {
        panic!("{what} missing or empty: {}\n  → {how}\n  (or set FASTCOPY_ALLOW_PLACEHOLDERS=1 to build anyway with an empty placeholder)", path.display());
    }
    if !path.exists() {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(path, b"");
    }
    println!("cargo:warning={what} missing: using an empty placeholder at {} ({how})", path.display());
}

fn main() {
    println!("cargo:rerun-if-env-changed=FASTCOPY_ALLOW_PLACEHOLDERS");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let triple = std::env::var("TARGET").unwrap_or_default();
    let windows = triple.contains("windows");
    let ext = if windows { ".exe" } else { "" };

    ensure(
        &manifest.join("binaries").join(format!("fc-helper-{triple}{ext}")),
        "fc-helper sidecar",
        "run `node app/scripts/prepare-sidecar.mjs` (done automatically by `tauri build` / `tauri dev`)",
    );

    // Resources declared in tauri.windows.conf.json (Windows 11 modern context menu).
    if windows {
        ensure(
            &manifest.join("../../target/release/fc_shellext_win.dll"),
            "fc_shellext_win.dll",
            "cargo build -p fc-shellext-win --release",
        );
        // The CLI ships next to the GUI (installed on the user's PATH).
        ensure(
            &manifest.join("../../target/release/fcopy.exe"),
            "fcopy.exe",
            "cargo build -p fc-cli --release",
        );
    }

    tauri_build::build()
}
