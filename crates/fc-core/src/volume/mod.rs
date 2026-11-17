//! Volume / drive detection: filesystem type, free space, bus (USB…), physical disk,
//! listing and safe ejection.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

#[cfg(all(unix, not(target_os = "macos")))]
mod other;
#[cfg(all(unix, not(target_os = "macos")))]
pub use self::other::*;

/// Filesystem family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsKind {
    Fat32,
    Fat,
    ExFat,
    Ntfs,
    ReFs,
    Apfs,
    Hfs,
    Network,
    Other(String),
}

impl FsKind {
    pub fn from_name(name: &str) -> FsKind {
        match name.to_ascii_lowercase().as_str() {
            "fat32" | "msdos" | "vfat" => FsKind::Fat32,
            "fat" | "fat16" | "fat12" => FsKind::Fat,
            "exfat" => FsKind::ExFat,
            "ntfs" | "ntfs3" | "fuseblk" => FsKind::Ntfs,
            "refs" => FsKind::ReFs,
            "apfs" => FsKind::Apfs,
            "hfs" | "hfs+" | "hfsplus" => FsKind::Hfs,
            "smbfs" | "nfs" | "afpfs" | "webdav" | "cifs" | "smb2" => FsKind::Network,
            other => FsKind::Other(other.to_string()),
        }
    }

    /// Largest single file the filesystem can store.
    pub fn max_file_size(&self) -> u64 {
        match self {
            FsKind::Fat32 => 4_294_967_295,
            FsKind::Fat => 2_147_483_647,
            _ => u64::MAX,
        }
    }

    /// Windows-style naming rules apply (forbidden chars, reserved names).
    pub fn windows_names(&self) -> bool {
        matches!(
            self,
            FsKind::Fat32
                | FsKind::Fat
                | FsKind::ExFat
                | FsKind::Ntfs
                | FsKind::ReFs
                | FsKind::Network
        ) || cfg!(windows)
    }

    /// No journal: unplugging mid-write may corrupt the volume.
    pub fn is_fat_family(&self) -> bool {
        matches!(self, FsKind::Fat32 | FsKind::Fat | FsKind::ExFat)
    }

    /// Supports extended attributes / ADS without polluting the volume.
    pub fn supports_xattrs(&self) -> bool {
        matches!(
            self,
            FsKind::Apfs | FsKind::Hfs | FsKind::Ntfs | FsKind::ReFs
        )
    }

    /// Supports copy-on-write cloning.
    pub fn supports_clone(&self) -> bool {
        matches!(self, FsKind::Apfs | FsKind::ReFs)
    }

    /// Timestamp comparison tolerance in milliseconds.
    pub fn mtime_tolerance_ms(&self) -> u64 {
        match self {
            FsKind::Fat32 | FsKind::Fat => 2000,
            FsKind::ExFat => 10,
            FsKind::Hfs => 1000,
            _ => 1,
        }
    }

    pub fn label(&self) -> String {
        match self {
            FsKind::Fat32 => "FAT32".into(),
            FsKind::Fat => "FAT".into(),
            FsKind::ExFat => "exFAT".into(),
            FsKind::Ntfs => "NTFS".into(),
            FsKind::ReFs => "ReFS".into(),
            FsKind::Apfs => "APFS".into(),
            FsKind::Hfs => "HFS+".into(),
            FsKind::Network => "Réseau".into(),
            FsKind::Other(s) => s.clone(),
        }
    }
}

/// Bus the storage device is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BusKind {
    Usb,
    Sd,
    Sata,
    Nvme,
    Scsi,
    Thunderbolt,
    Virtual,
    Network,
    Unknown,
}

/// Information about the volume holding a path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    /// Mount point / root (e.g. `E:\` or `/Volumes/KEY`).
    pub root: PathBuf,
    pub fs: FsKind,
    pub label: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub cluster_size: u64,
    pub read_only: bool,
    pub removable: bool,
    pub bus: BusKind,
    /// Device has a seek penalty (spinning disk). None = unknown.
    pub rotational: Option<bool>,
    /// Physical disk identifier (`3` for PhysicalDrive3, `disk4` on macOS).
    pub disk_id: Option<String>,
    /// Vendor/product string of the device.
    pub model: String,
    /// True if this volume hosts the running operating system.
    pub is_system: bool,
}

impl VolumeInfo {
    /// External, user-pluggable storage (USB stick, SD card, USB disk).
    pub fn is_external(&self) -> bool {
        self.removable || matches!(self.bus, BusKind::Usb | BusKind::Sd | BusKind::Thunderbolt)
    }

    pub fn is_network(&self) -> bool {
        self.fs == FsKind::Network || self.bus == BusKind::Network
    }

    /// Same physical device as `other`.
    pub fn same_disk(&self, other: &VolumeInfo) -> bool {
        match (&self.disk_id, &other.disk_id) {
            (Some(a), Some(b)) => a == b,
            _ => self.root == other.root,
        }
    }

    pub fn same_volume(&self, other: &VolumeInfo) -> bool {
        self.root == other.root
    }
}

/// Closest existing ancestor of `p` (the destination may not exist yet).
pub fn existing_ancestor(p: &Path) -> PathBuf {
    let mut cur = p.to_path_buf();
    loop {
        if cur.exists() {
            return cur;
        }
        if !cur.pop() {
            return p.to_path_buf();
        }
    }
}

/// Recommended exFAT cluster size for a drive of `total` bytes (bytes).
/// Never above 1 MiB: macOS refuses to mount exFAT with clusters ≥ 2 MiB.
pub fn recommended_exfat_cluster(total: u64, mostly_small_files: bool) -> u64 {
    if mostly_small_files || total <= 8 * (1 << 30) {
        32 * 1024
    } else {
        128 * 1024
    }
}

/// Fallback when detection fails: treat as an unknown local volume.
pub fn unknown_volume(p: &Path) -> VolumeInfo {
    VolumeInfo {
        root: existing_ancestor(p),
        fs: FsKind::Other("inconnu".into()),
        label: String::new(),
        total_bytes: 0,
        free_bytes: u64::MAX,
        cluster_size: 4096,
        read_only: false,
        removable: false,
        bus: BusKind::Unknown,
        rotational: None,
        disk_id: None,
        model: String::new(),
        is_system: false,
    }
}

/// Detect, falling back to an "unknown" description on error.
pub fn detect(p: &Path) -> VolumeInfo {
    volume_info(p).unwrap_or_else(|_| unknown_volume(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fs_names() {
        assert_eq!(FsKind::from_name("FAT32"), FsKind::Fat32);
        assert_eq!(FsKind::from_name("msdos"), FsKind::Fat32);
        assert_eq!(FsKind::from_name("exFAT"), FsKind::ExFat);
        assert_eq!(FsKind::Fat32.max_file_size(), 4_294_967_295);
    }
    #[test]
    fn detect_temp() {
        let v = detect(&std::env::temp_dir());
        assert!(v.free_bytes > 0);
    }
}
