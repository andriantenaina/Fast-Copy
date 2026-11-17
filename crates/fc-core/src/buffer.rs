//! Page-aligned I/O buffers, required for unbuffered (cache-bypassing) I/O on Windows.

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// Alignment used for every buffer and every unbuffered transfer size.
/// 4096 is a multiple of every common physical sector size (512 / 4096).
pub const ALIGN: usize = 4096;

/// A heap buffer whose address is aligned to [`ALIGN`].
pub struct AlignedBuf {
    ptr: NonNull<u8>,
    len: usize,
}

// SAFETY: AlignedBuf owns its allocation exclusively, like a Vec<u8>.
unsafe impl Send for AlignedBuf {}
unsafe impl Sync for AlignedBuf {}

impl AlignedBuf {
    /// Allocate a zeroed buffer; `len` is rounded up to a multiple of [`ALIGN`].
    pub fn new(len: usize) -> Self {
        let len = round_up(len.max(ALIGN), ALIGN);
        let layout = Layout::from_size_align(len, ALIGN).expect("layout");
        // SAFETY: layout has non-zero size.
        let ptr = unsafe { alloc_zeroed(layout) };
        let ptr = NonNull::new(ptr).unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
        AlignedBuf { ptr, len }
    }

    pub fn capacity(&self) -> usize {
        self.len
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.len, ALIGN).expect("layout");
        // SAFETY: allocated in `new` with the same layout.
        unsafe { dealloc(self.ptr.as_ptr(), layout) }
    }
}

impl Deref for AlignedBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: ptr is valid for len bytes for the lifetime of self.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: unique access through &mut self.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

#[inline]
pub fn round_up(v: usize, align: usize) -> usize {
    v.div_ceil(align) * align
}

#[inline]
pub fn round_up_u64(v: u64, align: u64) -> u64 {
    v.div_ceil(align) * align
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligned() {
        let b = AlignedBuf::new(10);
        assert_eq!(b.as_ptr() as usize % ALIGN, 0);
        assert_eq!(b.capacity(), ALIGN);
        assert!(b.iter().all(|&x| x == 0));
    }
}
