//! Minimal implementation for other Unixes (Linux), used for development and CI.
use super::{existing_ancestor, BusKind, FsKind, VolumeInfo};
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub fn volume_root(p: &Path) -> io::Result<PathBuf> {
    // Longest mount point prefix from /proc/mounts.
    let p = existing_ancestor(p).canonicalize()?;
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut best = PathBuf::from("/");
    for line in mounts.lines() {
        if let Some(mp) = line.split_whitespace().nth(1) {
            let mp = PathBuf::from(mp.replace("\\040", " "));
            if p.starts_with(&mp) && mp.as_os_str().len() > best.as_os_str().len() {
                best = mp;
            }
        }
    }
    Ok(best)
}

fn fs_type(root: &Path) -> String {
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    for line in mounts.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() > 2 && Path::new(&f[1].replace("\\040", " ")) == root {
            return f[2].to_string();
        }
    }
    String::new()
}

pub fn volume_info(p: &Path) -> io::Result<VolumeInfo> {
    let root = volume_root(p)?;
    let c = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid args.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(VolumeInfo {
        fs: FsKind::from_name(&fs_type(&root)),
        label: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        total_bytes: s.f_blocks as u64 * s.f_frsize as u64,
        free_bytes: s.f_bavail as u64 * s.f_frsize as u64,
        cluster_size: (s.f_bsize as u64).max(512),
        read_only: s.f_flag & libc::ST_RDONLY != 0,
        removable: root.starts_with("/media") || root.starts_with("/run/media"),
        bus: BusKind::Unknown,
        rotational: None,
        disk_id: None,
        model: String::new(),
        is_system: root == Path::new("/"),
        root,
    })
}

pub fn list_volumes() -> Vec<VolumeInfo> {
    volume_info(Path::new("/")).into_iter().collect()
}

pub fn eject(volume: &Path) -> io::Result<()> {
    let st = std::process::Command::new("udisksctl")
        .args(["unmount", "-b"])
        .arg(volume)
        .status()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::other("échec de l'éjection"))
    }
}
