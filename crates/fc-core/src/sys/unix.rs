use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// macOS F_NOCACHE has no alignment requirement.
pub const NOCACHE_NEEDS_ALIGNMENT: bool = false;

pub fn long_path(p: &Path) -> PathBuf {
    p.to_path_buf()
}

#[cfg(target_os = "macos")]
fn set_nocache(f: &File) {
    // SAFETY: valid fd; F_NOCACHE takes an int.
    unsafe {
        libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_nocache(f: &File) {
    // Best effort on other Unixes: hint sequential access.
    #[cfg(target_os = "linux")]
    unsafe {
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = f;
}

pub fn open_read(path: &Path, nocache: bool) -> io::Result<File> {
    let f = File::open(path)?;
    if nocache {
        set_nocache(&f);
    }
    Ok(f)
}

pub fn open_write_new(path: &Path, nocache: bool) -> io::Result<File> {
    let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
    if nocache {
        set_nocache(&f);
    }
    Ok(f)
}

pub fn open_write_existing(path: &Path, nocache: bool) -> io::Result<File> {
    let f = OpenOptions::new().read(true).write(true).open(path)?;
    if nocache {
        set_nocache(&f);
    }
    Ok(f)
}

#[inline]
pub fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    f.read_at(buf, offset)
}

#[inline]
pub fn write_at(f: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    f.write_at(buf, offset)
}

#[cfg(target_os = "macos")]
pub fn preallocate(f: &File, size: u64) -> io::Result<()> {
    if size == 0 {
        return Ok(());
    }
    let mut store = libc::fstore_t {
        fst_flags: libc::F_ALLOCATECONTIG,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: size as libc::off_t,
        fst_bytesalloc: 0,
    };
    // SAFETY: valid fd and struct.
    let mut r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_PREALLOCATE, &mut store) };
    if r == -1 {
        store.fst_flags = libc::F_ALLOCATEALL;
        r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_PREALLOCATE, &mut store) };
    }
    if r == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn preallocate(f: &File, size: u64) -> io::Result<()> {
    if size == 0 {
        return Ok(());
    }
    // SAFETY: valid fd.
    let r = unsafe { libc::fallocate(f.as_raw_fd(), libc::FALLOC_FL_KEEP_SIZE, 0, size as libc::off_t) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn preallocate(_f: &File, _size: u64) -> io::Result<()> {
    Ok(())
}

/// Durable flush. On macOS plain fsync only reaches the drive cache: use F_FULLFSYNC.
pub fn full_sync(f: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: valid fd.
        if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC) } != -1 {
            return Ok(());
        }
    }
    f.sync_all()
}

/// fsync a directory so renames inside it are durable.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    let f = File::open(dir)?;
    match f.sync_all() {
        Ok(()) => Ok(()),
        // Some filesystems (exFAT/msdos on macOS) refuse fsync on directories.
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOTSUP) | Some(libc::EBADF)) => Ok(()),
        Err(e) => Err(e),
    }
}

pub const KEPT_ATTRIBUTES: u32 = 0;
pub fn attributes(_meta: &std::fs::Metadata) -> u32 {
    0
}
pub fn set_attributes(_path: &Path, _attrs: u32) -> io::Result<()> {
    Ok(())
}
pub fn set_hidden(_path: &Path) -> io::Result<()> {
    Ok(())
}
pub fn clear_readonly(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    let mut p = meta.permissions();
    if p.mode() & 0o200 == 0 {
        p.set_mode(p.mode() | 0o200);
        std::fs::set_permissions(path, p)?;
    }
    Ok(())
}
pub fn list_streams(_path: &Path) -> Vec<String> {
    Vec::new()
}
pub fn list_streams_handle(_f: &File) -> Vec<String> {
    Vec::new()
}
pub fn copy_streams(_src: &Path, _dst: &Path, _streams: &[String]) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn cpath(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL dans le chemin"))
}

/// Extended attribute names (macOS).
#[cfg(target_os = "macos")]
pub fn list_xattrs(path: &Path) -> Vec<String> {
    let Ok(c) = cpath(path) else { return Vec::new() };
    // SAFETY: querying size with null buffer.
    let size = unsafe { libc::listxattr(c.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW) };
    if size <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size as usize];
    let n = unsafe { libc::listxattr(c.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), libc::XATTR_NOFOLLOW) };
    if n <= 0 {
        return Vec::new();
    }
    buf.truncate(n as usize);
    buf.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// Copy all extended attributes (incl. resource forks, Finder info). Returns count copied.
#[cfg(target_os = "macos")]
pub fn copy_xattrs(src: &Path, dst: &Path) -> io::Result<usize> {
    let names = list_xattrs(src);
    let cs = cpath(src)?;
    let cd = cpath(dst)?;
    let mut n = 0;
    for name in names {
        let Ok(cn) = CString::new(name) else { continue };
        // SAFETY: size query.
        let size = unsafe { libc::getxattr(cs.as_ptr(), cn.as_ptr(), std::ptr::null_mut(), 0, 0, libc::XATTR_NOFOLLOW) };
        if size < 0 {
            continue;
        }
        let mut buf = vec![0u8; size as usize];
        let got = unsafe {
            libc::getxattr(cs.as_ptr(), cn.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), 0, libc::XATTR_NOFOLLOW)
        };
        if got < 0 {
            continue;
        }
        let r = unsafe {
            libc::setxattr(cd.as_ptr(), cn.as_ptr(), buf.as_ptr() as *const _, got as usize, 0, libc::XATTR_NOFOLLOW)
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        n += 1;
    }
    Ok(n)
}

#[cfg(not(target_os = "macos"))]
pub fn list_xattrs(_path: &Path) -> Vec<String> {
    Vec::new()
}
#[cfg(not(target_os = "macos"))]
pub fn copy_xattrs(_src: &Path, _dst: &Path) -> io::Result<usize> {
    let _ = CString::new("");
    Ok(0)
}

/// Set a directory's modification time.
pub fn set_dir_mtime(dir: &Path, mtime: std::time::SystemTime) -> io::Result<()> {
    let f = File::open(dir)?;
    f.set_times(std::fs::FileTimes::new().set_modified(mtime))
}
