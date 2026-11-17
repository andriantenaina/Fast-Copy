use super::{existing_ancestor, BusKind, FsKind, VolumeInfo};
use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cstr_field(f: &[libc::c_char]) -> String {
    // SAFETY: statfs strings are NUL-terminated within the array.
    unsafe { CStr::from_ptr(f.as_ptr()) }.to_string_lossy().into_owned()
}

fn statfs(p: &Path) -> io::Result<libc::statfs> {
    let c = CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid path and out struct.
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(s)
}

/// `diskutil info -plist <dev>` as a dictionary.
pub(crate) fn diskutil_info(dev: &str) -> Option<plist::Dictionary> {
    let out = Command::new("/usr/sbin/diskutil").args(["info", "-plist", dev]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    plist::Value::from_reader_xml(&out.stdout[..]).ok()?.into_dictionary()
}

fn dict_bool(d: &plist::Dictionary, k: &str) -> Option<bool> {
    d.get(k).and_then(|v| v.as_boolean())
}
fn dict_str(d: &plist::Dictionary, k: &str) -> Option<String> {
    d.get(k).and_then(|v| v.as_string()).map(|s| s.to_string())
}

pub fn volume_root(p: &Path) -> io::Result<PathBuf> {
    let s = statfs(&existing_ancestor(p))?;
    Ok(PathBuf::from(cstr_field(&s.f_mntonname)))
}

pub fn volume_info(p: &Path) -> io::Result<VolumeInfo> {
    let s = statfs(&existing_ancestor(p))?;
    let fsname = cstr_field(&s.f_fstypename);
    let root = PathBuf::from(cstr_field(&s.f_mntonname));
    let from = cstr_field(&s.f_mntfromname);
    let mut info = VolumeInfo {
        root: root.clone(),
        fs: FsKind::from_name(&fsname),
        label: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        total_bytes: s.f_blocks as u64 * s.f_bsize as u64,
        free_bytes: s.f_bavail as u64 * s.f_bsize as u64,
        cluster_size: (s.f_bsize as u64).max(512),
        read_only: s.f_flags & libc::MNT_RDONLY as u32 != 0,
        removable: false,
        bus: BusKind::Unknown,
        rotational: None,
        disk_id: None,
        model: String::new(),
        is_system: root == Path::new("/") || root.starts_with("/System/Volumes"),
    };
    if s.f_flags & libc::MNT_LOCAL as u32 == 0 {
        info.bus = BusKind::Network;
        info.fs = FsKind::Network;
        return Ok(info);
    }
    if from.starts_with("/dev/") {
        if let Some(d) = diskutil_info(&from) {
            let proto = dict_str(&d, "BusProtocol").unwrap_or_default();
            info.bus = match proto.as_str() {
                "USB" => BusKind::Usb,
                "Secure Digital" | "SD" => BusKind::Sd,
                "SATA" => BusKind::Sata,
                "PCI-Express" | "Apple Fabric" | "NVMe" => BusKind::Nvme,
                "Thunderbolt" => BusKind::Thunderbolt,
                "Disk Image" | "Virtual Interface" => BusKind::Virtual,
                _ => BusKind::Unknown,
            };
            let internal = dict_bool(&d, "Internal").unwrap_or(true);
            info.removable = !internal
                || dict_bool(&d, "RemovableMedia").unwrap_or(false)
                || dict_bool(&d, "Removable").unwrap_or(false)
                || dict_bool(&d, "Ejectable").unwrap_or(false);
            info.rotational = dict_bool(&d, "SolidState").map(|ss| !ss);
            info.disk_id = dict_str(&d, "ParentWholeDisk");
            info.model = dict_str(&d, "MediaName").unwrap_or_default();
            if let Some(name) = dict_str(&d, "VolumeName") {
                if !name.is_empty() {
                    info.label = name;
                }
            }
            if let Some(b) = d.get("VolumeAllocationBlockSize").and_then(|v| v.as_unsigned_integer()) {
                info.cluster_size = b.max(512);
            }
        }
    }
    Ok(info)
}

pub fn list_volumes() -> Vec<VolumeInfo> {
    let mut out = vec![];
    if let Ok(v) = volume_info(Path::new("/")) {
        out.push(v);
    }
    if let Ok(rd) = std::fs::read_dir("/Volumes") {
        for e in rd.flatten() {
            if let Ok(v) = volume_info(&e.path()) {
                if v.root == e.path() {
                    out.push(v);
                }
            }
        }
    }
    out
}

/// Unmount and eject the whole disk holding `volume`.
pub fn eject(volume: &Path) -> io::Result<()> {
    let info = volume_info(volume)?;
    if info.is_system {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "refus : disque système"));
    }
    let target = info.disk_id.clone().unwrap_or_else(|| volume.to_string_lossy().into_owned());
    let out = Command::new("/usr/sbin/diskutil").args(["eject", &target]).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}
