//! Thin, platform-specific low-level file I/O helpers.
//!
//! * Windows: `FILE_FLAG_NO_BUFFERING` (cache bypass, needs aligned transfers),
//!   `FlushFileBuffers`, `FileAllocationInfo` preallocation, attributes, ADS.
//! * macOS: `F_NOCACHE`, `F_FULLFSYNC`, `F_PREALLOCATE`, xattrs.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use self::unix::*;

use std::fs::File;
use std::io;

/// Read until `buf` is full or EOF. Returns number of bytes read.
pub fn read_full_at(f: &File, buf: &mut [u8], mut offset: u64) -> io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        match read_at(f, &mut buf[done..], offset) {
            Ok(0) => break,
            Ok(n) => {
                let short = n < buf.len() - done;
                done += n;
                offset += n as u64;
                // On regular files a short read means end of file. Retrying would also issue
                // an unaligned request, which unbuffered handles reject.
                if short {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

/// Write the whole buffer at `offset`.
pub fn write_all_at(f: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match write_at(f, buf, offset) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "écriture nulle")),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
