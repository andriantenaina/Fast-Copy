use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FileAllocationInfo, FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
    FlushFileBuffers, SetFileAttributesW, SetFileInformationByHandle, FILE_ALLOCATION_INFO,
    FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_SYSTEM, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_NO_BUFFERING,
    FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    WIN32_FIND_STREAM_DATA,
};

/// Unbuffered I/O on Windows requires sector-aligned sizes and offsets.
pub const NOCACHE_NEEDS_ALIGNMENT: bool = true;

pub fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Convert to an extended-length path (`\\?\`) so paths > 260 chars work.
pub fn long_path(p: &Path) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        return p.to_path_buf();
    }
    if !p.is_absolute() {
        return p.to_path_buf();
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{rest}"))
    } else {
        PathBuf::from(format!(r"\\?\{s}"))
    }
}

/// Open a source file for reading. `nocache` bypasses the OS cache.
pub fn open_read(path: &Path, nocache: bool) -> io::Result<File> {
    let mut flags = FILE_FLAG_SEQUENTIAL_SCAN | FILE_FLAG_BACKUP_SEMANTICS;
    if nocache {
        flags |= FILE_FLAG_NO_BUFFERING;
    }
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(flags)
        .open(path)
}

/// Create (truncate) a destination file for writing.
pub fn open_write_new(path: &Path, nocache: bool) -> io::Result<File> {
    let mut flags = FILE_FLAG_SEQUENTIAL_SCAN;
    if nocache {
        flags |= FILE_FLAG_NO_BUFFERING;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(flags)
        .open(path)
}

/// Open an existing partial destination file for resuming (no truncation).
pub fn open_write_existing(path: &Path, nocache: bool) -> io::Result<File> {
    let mut flags = FILE_FLAG_SEQUENTIAL_SCAN;
    if nocache {
        flags |= FILE_FLAG_NO_BUFFERING;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(flags)
        .open(path)
}

#[inline]
pub fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    f.seek_read(buf, offset)
}

#[inline]
pub fn write_at(f: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    f.seek_write(buf, offset)
}

/// Reserve disk space without exposing stale data (never SetFileValidData).
pub fn preallocate(f: &File, size: u64) -> io::Result<()> {
    if size == 0 {
        return Ok(());
    }
    let info = FILE_ALLOCATION_INFO { AllocationSize: size as i64 };
    // SAFETY: valid handle, correctly sized struct.
    let ok = unsafe {
        SetFileInformationByHandle(
            f.as_raw_handle() as HANDLE,
            FileAllocationInfo,
            &info as *const _ as *const _,
            std::mem::size_of::<FILE_ALLOCATION_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Flush file data and the device write cache (FlushFileBuffers).
pub fn full_sync(f: &File) -> io::Result<()> {
    // SAFETY: valid handle.
    if unsafe { FlushFileBuffers(f.as_raw_handle() as HANDLE) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Directory entries are made durable by NTFS/FAT metadata flushes; nothing to do.
pub fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Windows file attributes we preserve.
pub const KEPT_ATTRIBUTES: u32 =
    FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM | FILE_ATTRIBUTE_ARCHIVE;

pub fn attributes(meta: &std::fs::Metadata) -> u32 {
    meta.file_attributes() & KEPT_ATTRIBUTES
}

pub fn set_attributes(path: &Path, attrs: u32) -> io::Result<()> {
    let a = if attrs == 0 { FILE_ATTRIBUTE_NORMAL } else { attrs };
    let w = wide(path.as_os_str());
    // SAFETY: valid NUL-terminated wide string.
    if unsafe { SetFileAttributesW(w.as_ptr(), a) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn set_hidden(path: &Path) -> io::Result<()> {
    set_attributes(path, FILE_ATTRIBUTE_HIDDEN)
}

/// Clear READONLY so a file can be replaced or deleted.
pub fn clear_readonly(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let a = meta.file_attributes();
    if a & FILE_ATTRIBUTE_READONLY != 0 {
        set_attributes(path, a & KEPT_ATTRIBUTES & !FILE_ATTRIBUTE_READONLY)?;
    }
    Ok(())
}

/// Named alternate data streams (excluding the default `::$DATA`).
/// Returns stream names like `:Zone.Identifier`.
pub fn list_streams(path: &Path) -> Vec<String> {
    let w = wide(path.as_os_str());
    let mut data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };
    // SAFETY: valid args; handle closed below.
    let h = unsafe {
        FindFirstStreamW(w.as_ptr(), FindStreamInfoStandard, &mut data as *mut _ as *mut _, 0)
    };
    let mut out = Vec::new();
    if h == INVALID_HANDLE_VALUE {
        let _ = unsafe { GetLastError() };
        return out;
    }
    loop {
        let len = data.cStreamName.iter().position(|&c| c == 0).unwrap_or(data.cStreamName.len());
        let name = String::from_utf16_lossy(&data.cStreamName[..len]);
        if name != "::$DATA" {
            if let Some(n) = name.strip_suffix(":$DATA") {
                out.push(n.to_string());
            }
        }
        // SAFETY: h valid.
        if unsafe { FindNextStreamW(h, &mut data as *mut _ as *mut _) } == 0 {
            break;
        }
    }
    unsafe { FindClose(h) };
    out
}

/// Named streams of an already-open file (no extra open).
pub fn list_streams_handle(f: &File) -> Vec<String> {
    use windows_sys::Win32::Storage::FileSystem::{FileStreamInfo, GetFileInformationByHandleEx, FILE_STREAM_INFO};
    let mut buf = vec![0u64; 8192]; // 64 KiB, 8-byte aligned
    // SAFETY: buffer large and aligned; parsed within bounds below.
    let ok = unsafe {
        GetFileInformationByHandleEx(f.as_raw_handle() as HANDLE, FileStreamInfo, buf.as_mut_ptr() as *mut _, (buf.len() * 8) as u32)
    };
    let mut out = Vec::new();
    if ok == 0 {
        return out; // FAT/exFAT and others have no streams
    }
    let base = buf.as_ptr() as *const u8;
    let total = buf.len() * 8;
    let mut off = 0usize;
    loop {
        if off + std::mem::size_of::<FILE_STREAM_INFO>() > total {
            break;
        }
        // SAFETY: offset checked against buffer size.
        let info = unsafe { &*(base.add(off) as *const FILE_STREAM_INFO) };
        let len = info.StreamNameLength as usize / 2;
        let name_ptr = std::ptr::addr_of!(info.StreamName) as *const u16;
        if off + std::mem::size_of::<FILE_STREAM_INFO>() + len * 2 > total + 2 {
            break;
        }
        let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name_ptr, len) });
        if name != "::$DATA" {
            if let Some(n) = name.strip_suffix(":$DATA") {
                out.push(n.to_string());
            }
        }
        if info.NextEntryOffset == 0 {
            break;
        }
        off += info.NextEntryOffset as usize;
    }
    out
}

/// Copy named alternate data streams from `src` to `dst`.
pub fn copy_streams(src: &Path, dst: &Path, streams: &[String]) -> io::Result<()> {
    for s in streams {
        let mut sp = src.as_os_str().to_owned();
        sp.push(s);
        let mut dp = dst.as_os_str().to_owned();
        dp.push(s);
        let data = std::fs::read(&sp)?;
        std::fs::write(&dp, data)?;
    }
    Ok(())
}

/// macOS-only concept; no-op on Windows.
pub fn list_xattrs(_path: &Path) -> Vec<String> {
    Vec::new()
}
pub fn copy_xattrs(_src: &Path, _dst: &Path) -> io::Result<usize> {
    Ok(0)
}

/// Set a directory's modification time (needs FILE_FLAG_BACKUP_SEMANTICS on Windows).
pub fn set_dir_mtime(dir: &Path, mtime: std::time::SystemTime) -> io::Result<()> {
    const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
    let f = OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)?;
    f.set_times(std::fs::FileTimes::new().set_modified(mtime))
}
