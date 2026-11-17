//! Small helpers.

use std::path::{Path, PathBuf};

/// `1.5 Go`, `320 Mo`… (French decimal-binary units, base 1024).
pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["o", "Ko", "Mo", "Go", "To", "Po"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} o")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub const PART_SUFFIX: &str = ".fcpart";

/// Hidden temporary name used while a file is being written: `.name.fcpart`.
/// Falls back to a hashed name when the result would exceed 255 UTF-16 units.
pub fn temp_path(final_path: &Path) -> PathBuf {
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut tmp = format!(".{name}{PART_SUFFIX}");
    if tmp.encode_utf16().count() > 255 {
        tmp = format!(
            ".{:016x}{PART_SUFFIX}",
            crate::hash::fingerprint(name.as_bytes()).0 as u64
        );
    }
    final_path.with_file_name(tmp)
}

pub fn is_temp_name(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(PART_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn human() {
        assert_eq!(human_bytes(512), "512 o");
        assert_eq!(human_bytes(1536), "1.5 Ko");
        assert_eq!(human_bytes(4 * 1024 * 1024 * 1024), "4.0 Go");
    }
    #[test]
    fn temp() {
        let p = temp_path(Path::new("/a/b/film.mkv"));
        assert_eq!(p, Path::new("/a/b/.film.mkv.fcpart"));
        assert!(is_temp_name(".film.mkv.fcpart"));
        let long = "x".repeat(250);
        let p = temp_path(&Path::new("/a").join(&long));
        assert!(p.file_name().unwrap().len() < 40);
    }
}
