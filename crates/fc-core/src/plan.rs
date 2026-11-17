//! Planning: walk sources, map destination names, detect devices, pre-check everything
//! *before* writing a single byte.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::buffer::round_up_u64;
use crate::error::{Error, Result};
use crate::names::{is_junk, sanitize_component, CollisionMap};
use crate::split::{SplitManifest, MANIFEST_SUFFIX};
use crate::types::{JobSpec, Mode, SplitMode};
use crate::volume::{self, BusKind, FsKind, VolumeInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Dir,
    File,
    Symlink,
    /// File larger than the destination FS allows → written as `.001`, `.002`… chunks.
    Split,
    /// Chunk set described by a `.fcsplit.json` manifest → reassembled.
    Join,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub kind: ItemKind,
    pub src: PathBuf,
    /// Source-relative path (display), '/' separated.
    pub rel: String,
    /// Destination-relative path after name sanitizing / collision handling.
    pub dst_rel: String,
    pub size: u64,
    pub mtime_ms: i64,
    /// Join: ordered chunk files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Blocks execution unless forced.
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum IssueKind {
    SourceMissing { path: String },
    DestInsideSource,
    InsufficientSpace { needed: u64, available: u64 },
    /// Files exceed the destination filesystem's limit (FAT32 4 GiB).
    FileTooLarge { count: u64, largest: u64, limit: u64 },
    /// Files will be split because of the FAT32 limit.
    WillSplit { count: u64 },
    /// Destination is read-only (e.g. NTFS drive on macOS).
    ReadOnlyDestination { fs: String },
    NamesChanged { count: u64 },
    /// FAT/exFAT has no journal: eject properly after copying.
    NoJournal { fs: String },
    WalkError { path: String, error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub severity: Severity,
    #[serde(flatten)]
    pub kind: IssueKind,
    pub message: String,
}

/// I/O tuning chosen from the source and destination devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Chunk size for large files (bytes).
    pub chunk: usize,
    /// Buffers in flight between reader and writer.
    pub depth: usize,
    /// Concurrent small-file workers.
    pub small_workers: usize,
    /// Files at least this large go through the pipelined large-file lane.
    pub large_threshold: u64,
    /// Bypass the OS cache on the destination.
    pub nocache_write: bool,
    /// Run the small-file pool and the large-file lane at the same time.
    pub parallel_lanes: bool,
    /// Flush the device every N small files (durability batch).
    pub sync_every: usize,
    /// Flush and journal a checkpoint every N bytes of a large file.
    pub checkpoint_bytes: u64,
    /// Verified modes: write small files through the OS cache and verify them as a batch
    /// (lets the OS coalesce writes; the cache-bypassing re-read forces them to the device).
    #[serde(default)]
    pub small_buffered: bool,
}

impl Profile {
    pub fn choose(src: &VolumeInfo, dst: &VolumeInfo, threads: Option<usize>) -> Profile {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let dst_slow = dst.is_external() || dst.rotational == Some(true);
        let src_hdd = src.rotational == Some(true);
        let mut p = if dst.is_network() || src.is_network() {
            Profile {
                name: "réseau".into(),
                chunk: 4 << 20,
                depth: 4,
                small_workers: 8,
                large_threshold: 1 << 20,
                nocache_write: false,
                parallel_lanes: true,
                sync_every: 64,
                checkpoint_bytes: 512 << 20,
                small_buffered: false,
            }
        } else if src.same_disk(dst) && (dst.rotational == Some(true) || src_hdd) {
            Profile {
                name: "même disque dur".into(),
                chunk: 32 << 20,
                depth: 2,
                small_workers: 1,
                large_threshold: 1 << 20,
                nocache_write: true,
                parallel_lanes: false,
                sync_every: 64,
                checkpoint_bytes: 512 << 20,
                small_buffered: false,
            }
        } else if dst_slow {
            let usb = matches!(dst.bus, BusKind::Usb | BusKind::Sd) || dst.removable;
            let hdd = dst.rotational == Some(true);
            Profile {
                name: if usb { "clé / disque USB".into() } else { "disque dur".into() },
                // Internal HDD: big sequential writes. USB flash: moderate chunks (erase blocks).
                chunk: if usb { 2 << 20 } else { 8 << 20 },
                depth: 4,
                // Internal HDD benefits from NCQ reordering; FAT on USB updates its table per file.
                small_workers: if usb { if hdd { 2 } else { 1 } } else { 4 },
                large_threshold: 1 << 20,
                nocache_write: true,
                parallel_lanes: false,
                sync_every: 256,
                checkpoint_bytes: 256 << 20,
                small_buffered: !hdd, // seeks: synchronous unbuffered writes win on spinning disks
            }
        } else {
            Profile {
                name: "SSD / NVMe".into(),
                chunk: 4 << 20,
                depth: 8,
                small_workers: if src_hdd { 2 } else { (cpus * 2).clamp(4, 16) },
                large_threshold: 1 << 20,
                nocache_write: true,
                parallel_lanes: !src_hdd,
                sync_every: 256,
                checkpoint_bytes: 1 << 30,
                small_buffered: true,
            }
        };
        if let Some(t) = threads {
            p.small_workers = t.max(1);
        }
        p
    }
}

/// A fully checked copy plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub spec: JobSpec,
    pub items: Vec<Item>,
    pub src_volume: VolumeInfo,
    pub dst_volume: VolumeInfo,
    pub profile: Profile,
    pub total_bytes: u64,
    pub total_files: u64,
    pub total_dirs: u64,
    pub issues: Vec<Issue>,
    /// (source-relative, destination-relative) for every renamed path.
    pub renamed: Vec<(String, String)>,
    /// Same volume + move → plain renames.
    pub same_volume: bool,
}

impl Plan {
    pub fn blocking_issues(&self) -> Vec<&Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error).collect()
    }

    pub fn dst_path(&self, item: &Item) -> PathBuf {
        join_rel(&self.spec.dest, &item.dst_rel)
    }
}

pub fn join_rel(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for c in rel.split('/') {
        if !c.is_empty() {
            p.push(c);
        }
    }
    p
}

pub fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn issue(severity: Severity, kind: IssueKind, message: impl Into<String>) -> Issue {
    Issue { severity, kind, message: message.into() }
}

fn human(b: u64) -> String {
    crate::util::human_bytes(b)
}

/// Build and check a plan for `spec`.
pub fn build(spec: &JobSpec) -> Result<Plan> {
    if spec.sources.is_empty() {
        return Err(Error::Other("aucune source".into()));
    }
    let mut issues = Vec::new();
    let mut raw: Vec<Item> = Vec::new();

    let dst_volume = volume::detect(&spec.dest);
    let src_volume = volume::detect(&spec.sources[0]);
    let dest_abs = absolutize(&spec.dest);

    for src in &spec.sources {
        let meta = match std::fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => {
                issues.push(issue(
                    Severity::Error,
                    IssueKind::SourceMissing { path: src.display().to_string() },
                    format!("source introuvable « {} » : {e}", src.display()),
                ));
                continue;
            }
        };
        let src_abs = absolutize(src);
        if meta.is_dir() && dest_abs.starts_with(&src_abs) {
            issues.push(issue(
                Severity::Error,
                IssueKind::DestInsideSource,
                format!("la destination est à l'intérieur de la source « {} »", src.display()),
            ));
            continue;
        }
        let base = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if meta.is_dir() {
            if !base.is_empty() {
                raw.push(Item { kind: ItemKind::Dir, src: src.clone(), rel: base.clone(), dst_rel: String::new(), size: 0, mtime_ms: mtime_ms(&meta), parts: vec![] });
            }
            let skip_junk = spec.skip_junk;
            let walker = walkdir::WalkDir::new(src)
                .min_depth(1)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(move |e| !(skip_junk && is_junk(&e.file_name().to_string_lossy())));
            for entry in walker {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        let path = e.path().map(|p| p.display().to_string()).unwrap_or_default();
                        issues.push(issue(Severity::Warning, IssueKind::WalkError { path: path.clone(), error: e.to_string() }, format!("lecture impossible « {path} » : {e}")));
                        continue;
                    }
                };
                let relp = entry.path().strip_prefix(src).unwrap_or(entry.path());
                let mut rel = base.clone();
                for c in relp.components() {
                    if !rel.is_empty() {
                        rel.push('/');
                    }
                    rel.push_str(&c.as_os_str().to_string_lossy());
                }
                let m = match entry.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        issues.push(issue(Severity::Warning, IssueKind::WalkError { path: rel.clone(), error: e.to_string() }, format!("métadonnées illisibles « {rel} » : {e}")));
                        continue;
                    }
                };
                let ft = entry.file_type();
                let kind = if ft.is_symlink() {
                    ItemKind::Symlink
                } else if ft.is_dir() {
                    ItemKind::Dir
                } else {
                    ItemKind::File
                };
                raw.push(Item {
                    kind,
                    src: entry.path().to_path_buf(),
                    rel,
                    dst_rel: String::new(),
                    size: if kind == ItemKind::File { m.len() } else { 0 },
                    mtime_ms: mtime_ms(&m),
                    parts: vec![],
                });
            }
        } else {
            let kind = if meta.file_type().is_symlink() { ItemKind::Symlink } else { ItemKind::File };
            raw.push(Item { kind, src: src.clone(), rel: base, dst_rel: String::new(), size: meta.len(), mtime_ms: mtime_ms(&meta), parts: vec![] });
        }
    }

    // Reassemble split sets (`x.fcsplit.json` + `x.001`…) when the destination can hold them.
    if spec.auto_join {
        let mut consumed: HashSet<PathBuf> = HashSet::new();
        let mut joins = Vec::new();
        for it in &raw {
            if it.kind == ItemKind::File && it.rel.ends_with(MANIFEST_SUFFIX) {
                if let Ok(m) = SplitManifest::load(&it.src) {
                    if m.total_size > spec.max_file_size_override.unwrap_or_else(|| dst_volume.fs.max_file_size()) {
                        continue;
                    }
                    let dir = it.src.parent().unwrap_or(Path::new("")).to_path_buf();
                    let parts: Vec<PathBuf> = m.parts.iter().map(|p| dir.join(&p.name)).collect();
                    if !parts.iter().all(|p| p.exists()) {
                        continue;
                    }
                    consumed.insert(it.src.clone());
                    for p in &parts {
                        consumed.insert(p.clone());
                    }
                    for extra in m.companion_files() {
                        consumed.insert(dir.join(extra));
                    }
                    let rel_dir = it.rel.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
                    joins.push(Item {
                        kind: ItemKind::Join,
                        src: it.src.clone(),
                        rel: format!("{rel_dir}{}", m.original_name),
                        dst_rel: String::new(),
                        size: m.total_size,
                        mtime_ms: m.mtime_ms,
                        parts,
                    });
                }
            }
        }
        if !consumed.is_empty() {
            raw.retain(|i| !consumed.contains(&i.src));
            raw.extend(joins);
        }
    }

    // Destination names.
    let win_rules = dst_volume.fs.windows_names();
    let mut collisions = CollisionMap::default();
    let mut renamed = Vec::new();
    let mut dir_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for it in raw.iter_mut() {
        let (parent, name) = match it.rel.rsplit_once('/') {
            Some((p, n)) => (Some(p.to_string()), n.to_string()),
            None => (None, it.rel.clone()),
        };
        let parent_dst = parent.as_ref().map(|p| dir_map.get(p).cloned().unwrap_or_else(|| sanitize_path(p, win_rules)));
        let clean = sanitize_component(&name, win_rules);
        let candidate = match &parent_dst {
            Some(p) if !p.is_empty() => format!("{p}/{clean}"),
            _ => clean,
        };
        let unique = collisions.unique(&candidate);
        if unique != it.rel {
            renamed.push((it.rel.clone(), unique.clone()));
        }
        if it.kind == ItemKind::Dir {
            dir_map.insert(it.rel.clone(), unique.clone());
        }
        it.dst_rel = unique;
    }

    // Size checks.
    let limit = spec.max_file_size_override.unwrap_or_else(|| dst_volume.fs.max_file_size());
    let oversize: Vec<&Item> = raw.iter().filter(|i| matches!(i.kind, ItemKind::File) && i.size > limit).collect();
    if !oversize.is_empty() {
        let largest = oversize.iter().map(|i| i.size).max().unwrap_or(0);
        let count = oversize.len() as u64;
        if spec.split == SplitMode::Auto && (dst_volume.fs.is_fat_family() || spec.max_file_size_override.is_some()) {
            issues.push(issue(
                Severity::Warning,
                IssueKind::WillSplit { count },
                format!(
                    "{count} fichier(s) dépassent la limite de {} du {} : ils seront découpés en morceaux (.001, .002…). Formater la clé en exFAT évite le découpage.",
                    human(limit),
                    dst_volume.fs.label()
                ),
            ));
            for it in raw.iter_mut() {
                if it.kind == ItemKind::File && it.size > limit {
                    it.kind = ItemKind::Split;
                }
            }
        } else {
            issues.push(issue(
                Severity::Error,
                IssueKind::FileTooLarge { count, largest, limit },
                format!(
                    "{count} fichier(s) dépassent la limite de {} du {} (le plus gros : {}). Formatez la destination en exFAT ou activez le découpage.",
                    human(limit),
                    dst_volume.fs.label(),
                    human(largest)
                ),
            ));
        }
    }

    if dst_volume.read_only {
        let msg = if dst_volume.fs == FsKind::Ntfs && cfg!(target_os = "macos") {
            "la destination est en NTFS : macOS ne peut que la lire. Formatez-la en exFAT pour l'utiliser sur Mac et sur Windows."
        } else {
            "la destination est en lecture seule."
        };
        issues.push(issue(Severity::Error, IssueKind::ReadOnlyDestination { fs: dst_volume.fs.label() }, msg));
    }

    let same_volume = spec.mode == Mode::Move && src_volume.same_volume(&dst_volume) && spec.sources.iter().all(|s| volume::detect(s).same_volume(&dst_volume));

    let total_files = raw.iter().filter(|i| matches!(i.kind, ItemKind::File | ItemKind::Split | ItemKind::Join)).count() as u64;
    let total_dirs = raw.iter().filter(|i| i.kind == ItemKind::Dir).count() as u64;
    let total_bytes: u64 = raw.iter().map(|i| i.size).sum();

    if !same_volume {
        let cluster = dst_volume.cluster_size.max(512);
        let needed: u64 = raw.iter().map(|i| round_up_u64(i.size, cluster)).sum::<u64>() + total_dirs * cluster;
        if dst_volume.free_bytes != u64::MAX && needed > dst_volume.free_bytes {
            issues.push(issue(
                Severity::Error,
                IssueKind::InsufficientSpace { needed, available: dst_volume.free_bytes },
                format!("espace insuffisant : {} nécessaires, {} disponibles sur la destination", human(needed), human(dst_volume.free_bytes)),
            ));
        }
    }

    if !renamed.is_empty() {
        issues.push(issue(
            Severity::Info,
            IssueKind::NamesChanged { count: renamed.len() as u64 },
            format!("{} nom(s) adapté(s) à la destination (caractères interdits, doublons de casse, Unicode)", renamed.len()),
        ));
    }
    if dst_volume.fs.is_fat_family() && dst_volume.is_external() {
        issues.push(issue(
            Severity::Info,
            IssueKind::NoJournal { fs: dst_volume.fs.label() },
            format!("{} n'a pas de journal : éjectez la clé proprement après la copie.", dst_volume.fs.label()),
        ));
    }

    let profile = Profile::choose(&src_volume, &dst_volume, spec.threads);
    Ok(Plan {
        id: uuid::Uuid::new_v4().to_string(),
        spec: spec.clone(),
        items: raw,
        src_volume,
        dst_volume,
        profile,
        total_bytes,
        total_files,
        total_dirs,
        issues,
        renamed,
        same_volume,
    })
}

fn sanitize_path(rel: &str, win: bool) -> String {
    rel.split('/').map(|c| sanitize_component(c, win)).collect::<Vec<_>>().join("/")
}

fn absolutize(p: &Path) -> PathBuf {
    let anc = volume::existing_ancestor(p);
    let canon = anc.canonicalize().unwrap_or(anc.clone());
    let rest = p.strip_prefix(&anc).unwrap_or(Path::new(""));
    let mut out = canon.join(rest);
    // Normalize Windows verbatim prefix for comparisons.
    let s = out.to_string_lossy().to_string();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        if !stripped.starts_with("UNC") {
            out = PathBuf::from(stripped);
        }
    }
    out
}
