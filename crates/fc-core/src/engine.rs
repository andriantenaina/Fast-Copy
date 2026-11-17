//! Low-level copy primitives: the pipelined reader → writer stream, cache-bypassing
//! verification, resume validation, metadata, atomic commit, split and join.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crossbeam_channel::bounded;

use crate::buffer::{round_up, AlignedBuf, ALIGN};
use crate::error::{Error, IoContext, Result};
use crate::hash::{Fingerprint, Hasher, Sha256};
use crate::plan::Profile;
use crate::split::{part_name, PartInfo, SplitManifest};
use crate::sys::{self, NOCACHE_NEEDS_ALIGNMENT};
use crate::types::{Control, Stats, VerifyMode};
use crate::util::temp_path;
use crate::volume::FsKind;

/// Shared context for copy primitives.
pub struct Ctx<'a> {
    pub profile: &'a Profile,
    pub control: &'a Control,
    pub stats: &'a Stats,
    pub verify: VerifyMode,
    pub dst_fs: &'a FsKind,
    pub warnings: &'a Mutex<Vec<String>>,
}

impl Ctx<'_> {
    pub fn warn(&self, msg: String) {
        self.warnings.lock().unwrap().push(msg);
    }
}

/// Open for reading, bypassing the cache when possible.
pub fn open_src(path: &Path, nocache: bool) -> Result<(File, bool)> {
    let lp = sys::long_path(path);
    if nocache {
        if let Ok(f) = sys::open_read(&lp, true) {
            return Ok((f, true));
        }
    }
    let f = sys::open_read(&lp, false).ctx("ouverture de la source", path)?;
    Ok((f, false))
}

fn read_request(len: usize, nocache: bool) -> usize {
    if nocache && NOCACHE_NEEDS_ALIGNMENT {
        round_up(len, ALIGN)
    } else {
        len
    }
}

/// Pipelined copy of `len` bytes from `src@src_off` to `dst@dst_off`.
///
/// A reader thread fills a ring of aligned buffers and hashes each one; this thread
/// writes them. Reads and writes overlap, the hash is free (computed from the same
/// buffer), and `on_ckpt(bytes_written_absolute)` is called every
/// `profile.checkpoint_bytes`.
#[allow(clippy::too_many_arguments)]
pub fn stream(
    ctx: &Ctx,
    src: &File,
    src_nocache: bool,
    src_path: &Path,
    src_off: u64,
    len: u64,
    dst: &File,
    dst_nocache: bool,
    dst_path: &Path,
    dst_off: u64,
    hasher: &mut Hasher,
    shas: &mut [&mut Sha256],
    on_ckpt: &mut dyn FnMut(u64) -> Result<()>,
) -> Result<()> {
    if len == 0 {
        return Ok(());
    }
    let chunk = ctx.profile.chunk.max(ALIGN);
    let depth = ctx.profile.depth.max(2);
    let (free_tx, free_rx) = bounded::<AlignedBuf>(depth);
    let (full_tx, full_rx) = bounded::<(AlignedBuf, usize)>(depth);
    let nbufs = depth.min(len.div_ceil(chunk as u64) as usize + 1);
    for _ in 0..nbufs {
        free_tx.send(AlignedBuf::new(chunk)).expect("pool");
    }

    std::thread::scope(|s| {
        // `move`: the reader owns `full_tx`, so the channel closes when it finishes.
        let reader = s.spawn(move || -> Result<()> {
            let mut pos = 0u64;
            while pos < len {
                ctx.control.checkpoint()?;
                let Ok(mut buf) = free_rx.recv() else {
                    return Ok(());
                };
                let want = (len - pos).min(chunk as u64) as usize;
                let req = read_request(want, src_nocache).min(buf.capacity());
                let n = sys::read_full_at(src, &mut buf[..req], src_off + pos)
                    .ctx("lecture", src_path)?;
                let used = n.min(want);
                if used < want {
                    // The file shrank while we were copying it.
                    return Err(Error::SourceChanged(src_path.to_path_buf()));
                }
                hasher.update(&buf[..used]);
                for s in shas.iter_mut() {
                    s.update(&buf[..used]);
                }
                pos += used as u64;
                if full_tx.send((buf, used)).is_err() {
                    return Ok(()); // writer stopped (error) — it reports
                }
            }
            Ok(())
        });

        let write_result = (|| -> Result<()> {
            let mut written = 0u64;
            let mut last_ckpt = 0u64;
            for (mut buf, n) in full_rx.iter() {
                let wlen = if dst_nocache && NOCACHE_NEEDS_ALIGNMENT {
                    let w = round_up(n, ALIGN);
                    buf[n..w].fill(0);
                    w
                } else {
                    n
                };
                sys::write_all_at(dst, &buf[..wlen], dst_off + written)
                    .ctx("écriture", dst_path)?;
                written += n as u64;
                ctx.stats.add_done(n as u64);
                if written - last_ckpt >= ctx.profile.checkpoint_bytes && written < len {
                    last_ckpt = written;
                    on_ckpt(dst_off + written)?;
                }
                let _ = free_tx.send(buf);
            }
            Ok(())
        })();
        drop(full_rx);
        drop(free_tx);
        let read_result = reader
            .join()
            .unwrap_or_else(|_| Err(Error::Other("thread de lecture interrompu".into())));
        // Reader errors (source changed / read error / cancel) take precedence: they explain
        // why the writer saw a short stream.
        read_result?;
        write_result
    })
}

/// Hash `len` bytes of `f` from `off` into `hasher`, reading in large aligned chunks.
// Hot verification path: each argument is a plain borrow/flag; bundling them into a struct
// would only add indirection for the six call sites.
#[allow(clippy::too_many_arguments)]
pub fn hash_into(
    ctx: &Ctx,
    f: &File,
    nocache: bool,
    path: &Path,
    off: u64,
    len: u64,
    hasher: &mut Hasher,
    count_verified: bool,
) -> Result<()> {
    let chunk = ctx.profile.chunk.max(ALIGN);
    let mut buf = AlignedBuf::new(chunk);
    let mut pos = 0u64;
    while pos < len {
        ctx.control.checkpoint()?;
        let want = (len - pos).min(chunk as u64) as usize;
        let req = read_request(want, nocache).min(buf.capacity());
        let n = sys::read_full_at(f, &mut buf[..req], off + pos).ctx("relecture", path)?;
        let used = n.min(want);
        if used < want {
            return Err(Error::VerifyMismatch(path.to_path_buf()));
        }
        hasher.update(&buf[..used]);
        pos += used as u64;
        if count_verified {
            ctx.stats.add_verified(used as u64);
        }
    }
    Ok(())
}

/// Re-read `path` from the device (cache bypassed) and compare to `expected`.
pub fn verify_file(ctx: &Ctx, path: &Path, size: u64, expected: Fingerprint) -> Result<()> {
    let (f, nocache) = open_src(path, true)?;
    let mut h = Hasher::new();
    hash_into(ctx, &f, nocache, path, 0, size, &mut h, true)?;
    let actual_len = f.metadata().ctx("relecture", path)?.len();
    if h.finish() != expected || actual_len != size {
        return Err(Error::VerifyMismatch(path.to_path_buf()));
    }
    Ok(())
}

/// Fingerprint a whole file (used by resume and the `verify` command).
pub fn fingerprint_file(ctx: &Ctx, path: &Path) -> Result<(u64, Fingerprint)> {
    let (f, nocache) = open_src(path, true)?;
    let size = f.metadata().ctx("lecture", path)?.len();
    let mut h = Hasher::new();
    hash_into(ctx, &f, nocache, path, 0, size, &mut h, false)?;
    Ok((size, h.finish()))
}

/// Source identity used to detect modification during the copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SrcStamp {
    pub size: u64,
    pub mtime: Option<SystemTime>,
}

pub fn stamp(meta: &std::fs::Metadata) -> SrcStamp {
    SrcStamp {
        size: meta.len(),
        mtime: meta.modified().ok(),
    }
}

/// A file fully written to its temporary path, not yet committed.
pub struct Written {
    pub tmp: PathBuf,
    pub file: File,
    pub hash: Fingerprint,
    pub size: u64,
    pub meta: std::fs::Metadata,
    /// `file` bypasses the cache: it can be re-read directly for verification.
    pub dst_nocache: bool,
    /// The temp file was marked hidden (attributes must be reset after commit).
    pub hidden: bool,
    /// Named alternate data streams of the source (Windows).
    pub streams: Vec<String>,
}

/// Write `src` into the hidden temp file next to `final_path`.
/// `resume_off`: durable checkpoint from the journal, re-validated before use.
/// `size_hint` (from the plan) selects the I/O path before opening; the real size is
/// read from the open handle (no extra open — every open costs an antivirus scan).
pub fn write_file(
    ctx: &Ctx,
    src_path: &Path,
    final_path: &Path,
    resume_off: Option<u64>,
    size_hint: u64,
    buffered: bool,
    on_ckpt: &mut dyn FnMut(u64) -> Result<()>,
) -> Result<Written> {
    let (src, src_nocache) = open_src(src_path, size_hint >= ctx.profile.large_threshold)?;
    let meta0 = src.metadata().ctx("lecture des métadonnées", src_path)?;
    let size = meta0.len();
    let large = size >= ctx.profile.large_threshold;
    let dst_nocache = ctx.profile.nocache_write && !buffered;
    let tmp = temp_path(final_path);
    let ltmp = sys::long_path(&tmp);

    let mut hasher = Hasher::new();
    let mut start = 0u64;
    let mut dst: Option<File> = None;

    if let Some(off) = resume_off {
        if off > 0 && off <= size && off % ALIGN as u64 == 0 {
            if let Ok(tlen) = std::fs::metadata(&ltmp).map(|m| m.len()) {
                if tlen >= off {
                    // Re-validate the prefix already on disk against the source.
                    let mut src_h = Hasher::new();
                    let mut tmp_h = Hasher::new();
                    let ok = (|| -> Result<bool> {
                        let (tf, tnc) = open_src(&tmp, true)?;
                        hash_into(ctx, &src, src_nocache, src_path, 0, off, &mut src_h, false)?;
                        hash_into(ctx, &tf, tnc, &tmp, 0, off, &mut tmp_h, false)?;
                        Ok(src_h.finish() == tmp_h.finish())
                    })()
                    .unwrap_or(false);
                    if ok {
                        if let Ok(f) = sys::open_write_existing(&ltmp, dst_nocache) {
                            hasher = src_h;
                            start = off;
                            ctx.stats.add_done(off);
                            dst = Some(f);
                        }
                    }
                }
            }
        }
    }

    let dst = match dst {
        Some(f) => f,
        None => {
            let f = match sys::open_write_new(&ltmp, dst_nocache) {
                Ok(f) => f,
                Err(e) if crate::error::is_disk_full(&e) => {
                    return Err(Error::io("création", &tmp, e))
                }
                Err(e) => return Err(Error::io("création", &tmp, e)),
            };
            if large {
                // Small temp files live for milliseconds: skip the extra calls.
                let _ = sys::set_hidden(&ltmp);
                let _ = sys::preallocate(&f, size);
            }
            f
        }
    };

    stream(
        ctx,
        &src,
        src_nocache,
        src_path,
        start,
        size - start,
        &dst,
        dst_nocache,
        &tmp,
        start,
        &mut hasher,
        &mut [],
        &mut |off| {
            sys::full_sync(&dst).ctx("vidage", &tmp)?;
            on_ckpt(off)
        },
    )?;
    dst.set_len(size).ctx("finalisation", &tmp)?;

    let meta1 = src.metadata().ctx("lecture des métadonnées", src_path)?;
    if stamp(&meta0) != stamp(&meta1) {
        return Err(Error::SourceChanged(src_path.to_path_buf()));
    }
    let streams = sys::list_streams_handle(&src);
    Ok(Written {
        tmp,
        file: dst,
        hash: hasher.finish(),
        size,
        meta: meta0,
        dst_nocache,
        hidden: large,
        streams,
    })
}

/// Verify per the configured mode (the temp file must already be flushed).
pub fn verify_written(ctx: &Ctx, w: &Written, src_path: &Path) -> Result<()> {
    match ctx.verify {
        VerifyMode::Fast => {
            ctx.stats.add_verified(w.size);
            Ok(())
        }
        VerifyMode::Full => verify_handle(ctx, w),
        VerifyMode::Paranoid => {
            verify_handle(ctx, w)?;
            let (f, nc) = open_src(src_path, true)?;
            let mut h = Hasher::new();
            hash_into(ctx, &f, nc, src_path, 0, w.size, &mut h, false)?;
            if h.finish() != w.hash {
                return Err(Error::SourceChanged(src_path.to_path_buf()));
            }
            Ok(())
        }
    }
}

/// Re-read the written data from the device. An unbuffered write handle is re-read
/// directly (saves an open); otherwise the file is reopened bypassing the cache.
fn verify_handle(ctx: &Ctx, w: &Written) -> Result<()> {
    if !w.dst_nocache {
        return verify_file(ctx, &w.tmp, w.size, w.hash);
    }
    let mut h = Hasher::new();
    hash_into(ctx, &w.file, true, &w.tmp, 0, w.size, &mut h, true)?;
    if h.finish() != w.hash {
        return Err(Error::VerifyMismatch(w.tmp.clone()));
    }
    Ok(())
}

/// Apply timestamps (incl. creation time where supported) through the open handle,
/// then extended attributes / alternate data streams when the destination supports them.
/// `streams`: the source's alternate data streams (Windows; empty elsewhere).
pub fn apply_metadata(
    ctx: &Ctx,
    file: &File,
    tmp: &Path,
    src_path: &Path,
    meta: &std::fs::Metadata,
    streams: &[String],
) {
    let mut times = std::fs::FileTimes::new();
    if let Ok(m) = meta.modified() {
        times = times.set_modified(m);
    }
    if let Ok(a) = meta.accessed() {
        times = times.set_accessed(a);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTimesExt;
        if let Ok(c) = meta.created() {
            times = times.set_created(c);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::FileTimesExt;
        if let Ok(c) = meta.created() {
            times = times.set_created(c);
        }
    }
    if let Err(e) = file.set_times(times) {
        ctx.warn(format!(
            "horodatage non appliqué à « {} » : {e}",
            tmp.display()
        ));
    }

    let lsrc = sys::long_path(src_path);
    let ltmp = sys::long_path(tmp);
    #[cfg(windows)]
    {
        if !streams.is_empty() {
            if matches!(ctx.dst_fs, FsKind::Ntfs | FsKind::ReFs) {
                if let Err(e) = sys::copy_streams(&lsrc, &ltmp, streams) {
                    ctx.warn(format!(
                        "flux de données alternatifs non copiés pour « {} » : {e}",
                        src_path.display()
                    ));
                }
            } else if streams.iter().any(|s| s != ":Zone.Identifier") {
                ctx.warn(format!(
                    "« {} » : {} flux de données alternatif(s) non conservé(s) ({} ne les supporte pas)",
                    src_path.display(),
                    streams.len(),
                    ctx.dst_fs.label()
                ));
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = streams;
        if ctx.dst_fs.supports_xattrs() {
            if let Err(e) = sys::copy_xattrs(&lsrc, &ltmp) {
                ctx.warn(format!(
                    "attributs étendus non copiés pour « {} » : {e}",
                    src_path.display()
                ));
            }
        } else {
            let names: Vec<String> = sys::list_xattrs(&lsrc)
                .into_iter()
                .filter(|n| {
                    n != "com.apple.quarantine"
                        && n != "com.apple.lastuseddate#PS"
                        && n != "com.apple.provenance"
                })
                .collect();
            if !names.is_empty() {
                ctx.warn(format!(
                    "« {} » : attributs étendus non conservés sur {} ({})",
                    src_path.display(),
                    ctx.dst_fs.label(),
                    names.join(", ")
                ));
            }
        }
    }
}

/// Attributes / permissions applied after the final rename.
/// `tmp_hidden`: the temp file was marked hidden and must be reset even if the source
/// has no special attributes.
pub fn apply_final_attributes(final_path: &Path, meta: &std::fs::Metadata, tmp_hidden: bool) {
    let lp = sys::long_path(final_path);
    #[cfg(windows)]
    {
        let a = sys::attributes(meta);
        // ARCHIVE alone is what a new file gets anyway: skip the extra open.
        if tmp_hidden || a & !0x20 != 0 {
            let _ = sys::set_attributes(&lp, a);
        }
    }
    #[cfg(not(windows))]
    let _ = tmp_hidden;
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(&lp, meta.permissions());
    }
}

/// Atomically move the verified temp file into place.
pub fn commit(tmp: &Path, final_path: &Path, replace: bool) -> Result<()> {
    let lf = sys::long_path(final_path);
    if replace && std::fs::symlink_metadata(&lf).is_ok() {
        let _ = sys::clear_readonly(&lf);
    }
    std::fs::rename(sys::long_path(tmp), &lf).ctx("renommage final", final_path)?;
    if let Some(dir) = final_path.parent() {
        let _ = sys::sync_dir(dir);
    }
    Ok(())
}

/// Remove a leftover temp file, ignoring errors.
pub fn discard_tmp(tmp: &Path) {
    let lp = sys::long_path(tmp);
    let _ = sys::clear_readonly(&lp);
    let _ = std::fs::remove_file(lp);
}

/// Try a copy-on-write clone (APFS / ReFS). Returns true on success.
pub fn try_clone(src: &Path, tmp: &Path) -> bool {
    reflink_copy::reflink(sys::long_path(src), sys::long_path(tmp)).is_ok()
}

/// Write an oversize file as chunks of `chunk` bytes next to `final_path`
/// (`name.001`, `name.002`…) plus manifest, `.sha256` and join scripts.
/// Every chunk is flushed, verified and committed atomically.
pub fn write_split(
    ctx: &Ctx,
    src_path: &Path,
    final_path: &Path,
    chunk: u64,
) -> Result<Vec<PathBuf>> {
    assert!(
        chunk.is_multiple_of(ALIGN as u64),
        "split chunk must be aligned"
    );
    let meta0 =
        std::fs::metadata(sys::long_path(src_path)).ctx("lecture des métadonnées", src_path)?;
    let size = meta0.len();
    let (src, src_nc) = open_src(src_path, true)?;
    let dir = final_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let count = size.div_ceil(chunk).max(1) as usize;
    let mut whole_sha = Sha256::new();
    let mut parts = Vec::with_capacity(count);
    let mut finals = Vec::with_capacity(count + 4);

    for k in 0..count {
        let off = k as u64 * chunk;
        let len = (size - off).min(chunk);
        let pname = part_name(&name, k, count);
        let pfinal = dir.join(&pname);
        let ptmp = temp_path(&pfinal);
        let lptmp = sys::long_path(&ptmp);
        let dst = sys::open_write_new(&lptmp, ctx.profile.nocache_write).ctx("création", &ptmp)?;
        let _ = sys::preallocate(&dst, len);
        let mut h = Hasher::new();
        let mut psha = Sha256::new();
        // One pass feeds the part hash, the part SHA-256 and the whole-file SHA-256.
        stream(
            ctx,
            &src,
            src_nc,
            src_path,
            off,
            len,
            &dst,
            ctx.profile.nocache_write,
            &ptmp,
            0,
            &mut h,
            &mut [&mut psha, &mut whole_sha],
            &mut |_| Ok(()),
        )?;
        dst.set_len(len).ctx("finalisation", &ptmp)?;
        sys::full_sync(&dst).ctx("vidage", &ptmp)?;
        if ctx.verify != VerifyMode::Fast {
            verify_file(ctx, &ptmp, len, h.finish())?;
        } else {
            ctx.stats.add_verified(len);
        }
        apply_metadata(ctx, &dst, &ptmp, src_path, &meta0, &[]);
        drop(dst);
        commit(&ptmp, &pfinal, true)?;
        parts.push(PartInfo {
            name: pname,
            size: len,
            sha256: psha.finish_hex(),
        });
        finals.push(pfinal);
    }
    let meta1 =
        std::fs::metadata(sys::long_path(src_path)).ctx("lecture des métadonnées", src_path)?;
    if stamp(&meta0) != stamp(&meta1) {
        return Err(Error::SourceChanged(src_path.to_path_buf()));
    }
    let manifest = SplitManifest {
        format: 1,
        tool: format!("Fast Copy {}", env!("CARGO_PKG_VERSION")),
        original_name: name,
        total_size: size,
        mtime_ms: crate::plan::mtime_ms(&meta0),
        chunk_size: chunk,
        parts,
        sha256: whole_sha.finish_hex(),
    };
    finals.extend(manifest.write_all(&dir)?);
    Ok(finals)
}

/// Reassemble a split set into `final_path`'s temp file, verifying every part's and the
/// whole file's SHA-256 against the manifest. Returns the written (unflushed) temp file.
pub fn write_join(
    ctx: &Ctx,
    manifest_path: &Path,
    parts: &[PathBuf],
    final_path: &Path,
) -> Result<Written> {
    let manifest = SplitManifest::load(manifest_path)?;
    let tmp = temp_path(final_path);
    let ltmp = sys::long_path(&tmp);
    // Unbuffered writes need every part boundary to be aligned.
    let aligned = manifest
        .parts
        .iter()
        .rev()
        .skip(1)
        .all(|p| p.size % ALIGN as u64 == 0);
    let nocache = ctx.profile.nocache_write && (aligned || !NOCACHE_NEEDS_ALIGNMENT);
    let dst = sys::open_write_new(&ltmp, nocache).ctx("création", &tmp)?;
    let _ = sys::set_hidden(&ltmp);
    let _ = sys::preallocate(&dst, manifest.total_size);
    let mut h = Hasher::new();
    let mut whole = Sha256::new();
    let mut off = 0u64;
    for (i, p) in parts.iter().enumerate() {
        let info = manifest
            .parts
            .get(i)
            .ok_or_else(|| Error::Other(format!("morceau inattendu « {} »", p.display())))?;
        let (src, nc) = open_src(p, true)?;
        let len = src.metadata().ctx("lecture", p)?.len();
        if len != info.size {
            return Err(Error::Other(format!(
                "taille du morceau « {} » incorrecte : {} au lieu de {}",
                p.display(),
                len,
                info.size
            )));
        }
        let mut psha = Sha256::new();
        // Whole-file SHA is computed over the reassembled stream below, part SHA here.
        stream(
            ctx,
            &src,
            nc,
            p,
            0,
            len,
            &dst,
            nocache,
            &tmp,
            off,
            &mut h,
            &mut [&mut psha],
            &mut |_| Ok(()),
        )?;
        if psha.finish_hex() != info.sha256 {
            discard_tmp(&tmp);
            return Err(Error::Other(format!(
                "le morceau « {} » est corrompu (SHA-256 différent du manifeste)",
                p.display()
            )));
        }
        off += len;
    }
    dst.set_len(off).ctx("finalisation", &tmp)?;
    sys::full_sync(&dst).ctx("vidage", &tmp)?;
    // Whole-file SHA-256 over the reassembled file, read back from the device.
    {
        let (rf, rnc) = open_src(&tmp, true)?;
        let chunk = ctx.profile.chunk.max(ALIGN);
        let mut buf = AlignedBuf::new(chunk);
        let mut pos = 0u64;
        while pos < off {
            ctx.control.checkpoint()?;
            let want = (off - pos).min(chunk as u64) as usize;
            let n = sys::read_full_at(&rf, &mut buf[..read_request(want, rnc)], pos)
                .ctx("relecture", &tmp)?;
            let used = n.min(want);
            if used == 0 {
                break;
            }
            whole.update(&buf[..used]);
            ctx.stats.add_verified(used as u64);
            pos += used as u64;
        }
    }
    if whole.finish_hex() != manifest.sha256 || off != manifest.total_size {
        discard_tmp(&tmp);
        return Err(Error::VerifyMismatch(final_path.to_path_buf()));
    }
    let meta = std::fs::metadata(&parts[0]).ctx("lecture", &parts[0])?;
    let mtime =
        SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(manifest.mtime_ms.max(0) as u64);
    let _ = dst.set_times(std::fs::FileTimes::new().set_modified(mtime));
    Ok(Written {
        tmp,
        file: dst,
        hash: h.finish(),
        size: off,
        meta,
        dst_nocache: nocache,
        hidden: true,
        streams: vec![],
    })
}
