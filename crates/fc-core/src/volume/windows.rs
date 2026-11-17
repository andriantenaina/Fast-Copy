use super::{existing_ancestor, BusKind, FsKind, VolumeInfo};
use crate::sys::wide;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Parent, CM_Request_Device_EjectW, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceProperty, StorageDeviceSeekPenaltyProperty,
    DEVICE_SEEK_PENALTY_DESCRIPTOR, GUID_DEVINTERFACE_DISK, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER,
    STORAGE_PROPERTY_QUERY,
};
use windows_sys::Win32::System::SystemServices::FILE_READ_ONLY_VOLUME;
use windows_sys::Win32::System::WindowsProgramming::{DRIVE_REMOTE, DRIVE_REMOVABLE};
use windows_sys::Win32::System::IO::DeviceIoControl;

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..len])
        .to_string_lossy()
        .into_owned()
}

/// Mount root of the volume containing `p` (e.g. `E:\`).
pub fn volume_root(p: &Path) -> io::Result<PathBuf> {
    let p = existing_ancestor(p);
    let w = wide(p.as_os_str());
    let mut out = [0u16; 1024];
    // SAFETY: buffers valid.
    if unsafe { GetVolumePathNameW(w.as_ptr(), out.as_mut_ptr(), out.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(from_wide(&out)))
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: handle owned.
        unsafe { CloseHandle(self.0) };
    }
}

fn open_device(path: &str) -> io::Result<Handle> {
    let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: access 0 = query only, no admin needed.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(h))
}

fn ioctl<T>(h: &Handle, code: u32, input: Option<&STORAGE_PROPERTY_QUERY>, out: &mut T) -> bool {
    let mut ret = 0u32;
    let (ip, il) = match input {
        Some(q) => (
            q as *const _ as *const _,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
        ),
        None => (std::ptr::null(), 0),
    };
    // SAFETY: buffers sized by type.
    unsafe {
        DeviceIoControl(
            h.0,
            code,
            ip,
            il,
            out as *mut T as *mut _,
            std::mem::size_of::<T>() as u32,
            &mut ret,
            std::ptr::null_mut(),
        ) != 0
    }
}

fn device_number(h: &Handle) -> Option<u32> {
    let mut n: STORAGE_DEVICE_NUMBER = unsafe { std::mem::zeroed() };
    ioctl(h, IOCTL_STORAGE_GET_DEVICE_NUMBER, None, &mut n).then_some(n.DeviceNumber)
}

struct DeviceProps {
    bus: BusKind,
    removable: bool,
    model: String,
    rotational: Option<bool>,
    disk: Option<u32>,
}

#[allow(non_upper_case_globals)]
fn device_props(h: &Handle) -> DeviceProps {
    let mut props = DeviceProps {
        bus: BusKind::Unknown,
        removable: false,
        model: String::new(),
        rotational: None,
        disk: None,
    };
    let q = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut buf = [0u8; 1024];
    let mut ret = 0u32;
    // SAFETY: buffer sized.
    let ok = unsafe {
        DeviceIoControl(
            h.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &q as *const _ as *const _,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        ) != 0
    };
    if ok && ret as usize >= std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        // SAFETY: buffer holds a descriptor (possibly unaligned → read_unaligned).
        let d: STORAGE_DEVICE_DESCRIPTOR =
            unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const _) };
        props.removable = d.RemovableMedia != 0;
        props.bus = match d.BusType {
            BusTypeUsb => BusKind::Usb,
            BusTypeSd | BusTypeMmc => BusKind::Sd,
            BusTypeSata | BusTypeAta => BusKind::Sata,
            BusTypeNvme => BusKind::Nvme,
            BusTypeScsi | BusTypeSas | BusTypeRAID | BusTypeSpaces => BusKind::Scsi,
            BusTypeVirtual | BusTypeFileBackedVirtual => BusKind::Virtual,
            _ => BusKind::Unknown,
        };
        let s = |off: u32| -> String {
            if off == 0 || off as usize >= buf.len() {
                return String::new();
            }
            let tail = &buf[off as usize..];
            let end = tail.iter().position(|&c| c == 0).unwrap_or(tail.len());
            String::from_utf8_lossy(&tail[..end]).trim().to_string()
        };
        props.model = format!("{} {}", s(d.VendorIdOffset), s(d.ProductIdOffset))
            .trim()
            .to_string();
    }
    let q = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceSeekPenaltyProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut sp: DEVICE_SEEK_PENALTY_DESCRIPTOR = unsafe { std::mem::zeroed() };
    if ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, Some(&q), &mut sp) {
        props.rotational = Some(sp.IncursSeekPenalty != 0);
    }
    props.disk = device_number(h);
    props
}

fn system_disk() -> Option<u32> {
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let h = open_device(&format!(r"\\.\{sys}")).ok()?;
    device_number(&h)
}

/// Full description of the volume holding `p`.
pub fn volume_info(p: &Path) -> io::Result<VolumeInfo> {
    let root = volume_root(p)?;
    let wroot = wide(root.as_os_str());

    let mut label = [0u16; 261];
    let mut fsname = [0u16; 261];
    let (mut serial, mut maxlen, mut flags) = (0u32, 0u32, 0u32);
    // SAFETY: buffers valid.
    let ok = unsafe {
        GetVolumeInformationW(
            wroot.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            &mut serial,
            &mut maxlen,
            &mut flags,
            fsname.as_mut_ptr(),
            fsname.len() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(wroot.as_ptr(), &mut avail, &mut total, &mut free) };
    let (mut spc, mut bps, mut fc, mut tc) = (0u32, 0u32, 0u32, 0u32);
    let cluster = if unsafe {
        GetDiskFreeSpaceW(wroot.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc)
    } != 0
    {
        (spc as u64 * bps as u64).max(512)
    } else {
        4096
    };
    let drive_type = unsafe { GetDriveTypeW(wroot.as_ptr()) };
    let mut fs = FsKind::from_name(&from_wide(&fsname));

    let mut info = VolumeInfo {
        root: root.clone(),
        fs: fs.clone(),
        label: from_wide(&label),
        total_bytes: total,
        free_bytes: avail,
        cluster_size: cluster,
        read_only: flags & FILE_READ_ONLY_VOLUME != 0,
        removable: drive_type == DRIVE_REMOVABLE,
        bus: BusKind::Unknown,
        rotational: None,
        disk_id: None,
        model: String::new(),
        is_system: false,
    };

    if drive_type == DRIVE_REMOTE
        || root.to_string_lossy().starts_with(r"\\") && !root.to_string_lossy().starts_with(r"\\?\")
    {
        info.bus = BusKind::Network;
        if !matches!(fs, FsKind::Ntfs | FsKind::ReFs) {
            fs = FsKind::Network;
            info.fs = fs;
        }
        return Ok(info);
    }

    // Volume GUID path → device handle for storage queries.
    let mut vol = [0u16; 128];
    if unsafe {
        GetVolumeNameForVolumeMountPointW(wroot.as_ptr(), vol.as_mut_ptr(), vol.len() as u32)
    } != 0
    {
        let vname = from_wide(&vol);
        let dev = vname.trim_end_matches('\\');
        if let Ok(h) = open_device(dev) {
            let props = device_props(&h);
            info.bus = props.bus;
            info.removable |= props.removable;
            info.model = props.model;
            info.rotational = props.rotational;
            info.disk_id = props.disk.map(|d| d.to_string());
            if let (Some(d), Some(s)) = (props.disk, system_disk()) {
                info.is_system = d == s;
            }
        }
    }
    Ok(info)
}

/// All mounted drive letters.
pub fn list_volumes() -> Vec<VolumeInfo> {
    // SAFETY: no args.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = PathBuf::from(format!("{letter}:\\"));
        if let Ok(v) = volume_info(&root) {
            out.push(v);
        }
    }
    out
}

/// Safely eject the device holding `volume` ("Safely Remove Hardware"; no admin needed).
pub fn eject(volume: &Path) -> io::Result<()> {
    let info = volume_info(volume)?;
    let disk: u32 = info
        .disk_id
        .as_deref()
        .and_then(|d| d.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "disque physique introuvable"))?;
    if info.is_system {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refus : disque système",
        ));
    }
    let devinst = disk_devinst(disk)?;
    let mut parent = 0u32;
    // SAFETY: valid devinst.
    if unsafe { CM_Get_Parent(&mut parent, devinst, 0) } != CR_SUCCESS {
        return Err(io::Error::other("parent du périphérique introuvable"));
    }
    let mut last = String::new();
    for _ in 0..3 {
        let mut veto = 0i32;
        let mut name = [0u16; 260];
        // SAFETY: buffers valid.
        let r = unsafe {
            CM_Request_Device_EjectW(parent, &mut veto, name.as_mut_ptr(), name.len() as u32, 0)
        };
        if r == CR_SUCCESS && veto == 0 {
            return Ok(());
        }
        last = format!("éjection refusée (veto {veto}) par {}", from_wide(&name));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(io::Error::other(last))
}

/// Find the PnP device instance of PhysicalDrive<disk>.
fn disk_devinst(disk: u32) -> io::Result<u32> {
    // SAFETY: standard SetupAPI enumeration; set destroyed at end.
    unsafe {
        let set = SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_DISK,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if set as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut result = Err(io::Error::new(
            io::ErrorKind::NotFound,
            "périphérique disque introuvable",
        ));
        let mut idx = 0u32;
        loop {
            let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(
                set,
                std::ptr::null(),
                &GUID_DEVINTERFACE_DISK,
                idx,
                &mut iface,
            ) == 0
            {
                break;
            }
            idx += 1;
            let mut buf = vec![0u64; 512]; // 4 KiB, 8-byte aligned
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            let mut devinfo: SP_DEVINFO_DATA = std::mem::zeroed();
            devinfo.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                set,
                &iface,
                detail,
                (buf.len() * 8) as u32,
                std::ptr::null_mut(),
                &mut devinfo,
            ) == 0
            {
                continue;
            }
            let path_ptr = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
            let mut len = 0;
            while *path_ptr.add(len) != 0 {
                len += 1;
            }
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(path_ptr, len));
            if let Ok(h) = open_device(&path) {
                if device_number(&h) == Some(disk) {
                    result = Ok(devinfo.DevInst);
                    break;
                }
            }
        }
        SetupDiDestroyDeviceInfoList(set);
        result
    }
}
