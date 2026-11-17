use fc_core::journal::Journal;
use fc_core::plan::{self, ItemKind};
use fc_core::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn data(len: usize, seed: u8) -> Vec<u8> {
    let mut x: u32 = 0x9E37_79B9 ^ seed as u32;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

const SIZES: &[usize] = &[
    0,
    1,
    4095,
    4096,
    4097,
    65_536,
    1_048_575,
    1_048_576,
    1_048_589,
    9 * 1_048_576 + 7,
];

fn make_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut v = Vec::new();
    for (i, s) in SIZES.iter().enumerate() {
        let p = root
            .join("src")
            .join(format!("sub{}", i % 3))
            .join(format!("f{i}_{s}.bin"));
        let d = data(*s, i as u8);
        write(&p, &d);
        v.push((p, d));
    }
    let p = root.join("src").join("été").join("Ünïcødé ✓.txt");
    let d = b"bonjour".to_vec();
    write(&p, &d);
    v.push((p, d));
    write(&root.join("src").join(".DS_Store"), b"junk");
    std::fs::create_dir_all(root.join("src").join("empty_dir")).unwrap();
    v
}

fn quick_spec(srcs: Vec<PathBuf>, dest: PathBuf) -> JobSpec {
    let mut s = JobSpec::new(srcs, dest);
    s.on_conflict = OnConflict::Overwrite;
    s
}

fn run(spec: &JobSpec) -> Report {
    let p = plan::build(spec).unwrap();
    let dir = tempfile::tempdir().unwrap();
    job::execute(
        &p,
        &Control::new(),
        &NullSink,
        &NonInteractive,
        job::ExecOptions {
            journal_dir: Some(dir.path().into()),
            ..Default::default()
        },
    )
    .unwrap()
}

fn assert_no_temp(dir: &Path) {
    for e in walkdir(dir) {
        let n = e.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !util::is_temp_name(&n),
            "leftover temp file {}",
            e.display()
        );
    }
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            }
            out.push(p);
        }
    }
    out
}

#[test]
fn copies_tree_exactly() {
    let t = tempfile::tempdir().unwrap();
    let files = make_tree(t.path());
    let dest = t.path().join("dst");
    let r = run(&quick_spec(vec![t.path().join("src")], dest.clone()));
    assert_eq!(r.status, JobStatus::Completed, "{:?}", r.errors);
    assert_eq!(r.files_copied as usize, files.len());
    for (p, d) in &files {
        let rel = p.strip_prefix(t.path()).unwrap();
        let out = dest.join(rel);
        assert_eq!(
            &std::fs::read(&out).unwrap(),
            d,
            "content of {}",
            out.display()
        );
        let sm = std::fs::metadata(p).unwrap().modified().unwrap();
        let dm = std::fs::metadata(&out).unwrap().modified().unwrap();
        let diff = sm
            .duration_since(dm)
            .or_else(|_| dm.duration_since(sm))
            .unwrap();
        assert!(
            diff.as_millis() < 5,
            "mtime preserved for {}",
            out.display()
        );
    }
    assert!(!dest.join("src").join(".DS_Store").exists(), "junk skipped");
    assert!(dest.join("src").join("empty_dir").is_dir());
    assert_no_temp(&dest);
}

#[test]
fn verify_modes_and_single_file() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("one.bin");
    write(&src, &data(3_000_000, 7));
    for mode in [VerifyMode::Fast, VerifyMode::Full, VerifyMode::Paranoid] {
        let dest = t.path().join(format!("out_{mode:?}"));
        let mut spec = quick_spec(vec![src.clone()], dest.clone());
        spec.verify = mode;
        let r = run(&spec);
        assert_eq!(r.status, JobStatus::Completed);
        assert_eq!(
            std::fs::read(dest.join("one.bin")).unwrap(),
            std::fs::read(&src).unwrap()
        );
    }
}

#[test]
fn conflict_policies() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("a.txt");
    write(&src, b"new content");
    let dest = t.path().join("d");
    write(&dest.join("a.txt"), b"old");

    let mut spec = quick_spec(vec![src.clone()], dest.clone());
    spec.on_conflict = OnConflict::Skip;
    let r = run(&spec);
    assert_eq!(r.files_skipped, 1);
    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"old");

    spec.on_conflict = OnConflict::KeepBoth;
    run(&spec);
    assert_eq!(
        std::fs::read(dest.join("a (2).txt")).unwrap(),
        b"new content"
    );
    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"old");

    spec.on_conflict = OnConflict::IfDifferent;
    run(&spec);
    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"new content");
    // Now identical (size + mtime) → skipped.
    let r = run(&spec);
    assert_eq!(r.files_skipped, 1);

    spec.on_conflict = OnConflict::Overwrite;
    write(&src, b"newest!");
    run(&spec);
    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"newest!");
}

#[test]
fn ask_conflict_apply_to_all() {
    struct Asker(std::sync::Mutex<u32>);
    impl Interaction for Asker {
        fn resolve_conflict(&self, _i: &ConflictInfo) -> ConflictDecision {
            *self.0.lock().unwrap() += 1;
            ConflictDecision {
                action: OnConflict::Overwrite,
                apply_to_all: true,
            }
        }
        fn disk_full(&self, _: &str, _: u64, _: u64) -> DiskFullAction {
            DiskFullAction::Abort
        }
    }
    let t = tempfile::tempdir().unwrap();
    for i in 0..20 {
        write(&t.path().join("s").join(format!("{i}.txt")), b"new");
        write(
            &t.path().join("d").join("s").join(format!("{i}.txt")),
            b"old",
        );
    }
    let mut spec = JobSpec::new(vec![t.path().join("s")], t.path().join("d"));
    spec.on_conflict = OnConflict::Ask;
    let p = plan::build(&spec).unwrap();
    let asker = Asker(Default::default());
    let r = job::execute(
        &p,
        &Control::new(),
        &NullSink,
        &asker,
        job::ExecOptions {
            journal: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(*asker.0.lock().unwrap(), 1, "asked only once");
    assert_eq!(r.files_copied, 20);
    assert_eq!(std::fs::read(t.path().join("d/s/7.txt")).unwrap(), b"new");
}

#[test]
fn split_then_join_roundtrip() {
    let t = tempfile::tempdir().unwrap();
    let big = t.path().join("film.mkv");
    let content = data(1_048_576 + 5000, 3);
    write(&big, &content);
    let key = t.path().join("key");
    let mut spec = quick_spec(vec![big.clone()], key.clone());
    spec.max_file_size_override = Some(1_000_000);
    spec.split_chunk_override = Some(262_144);
    let p = plan::build(&spec).unwrap();
    assert_eq!(p.items[0].kind, ItemKind::Split);
    let r = job::execute(
        &p,
        &Control::new(),
        &NullSink,
        &NonInteractive,
        job::ExecOptions {
            journal: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.status, JobStatus::Completed, "{:?}", r.errors);
    for i in 1..=5 {
        assert!(key.join(format!("film.mkv.{i:03}")).exists(), "part {i}");
    }
    assert!(!key.join("film.mkv.006").exists());
    assert!(key.join("film.mkv.fcsplit.json").exists());
    assert!(key.join("film.mkv.sha256").exists());
    assert!(key.join("JOIN-film.mkv.cmd").exists());
    assert!(key.join("join-film.mkv.sh").exists());
    // Manual join equals original.
    let mut joined = Vec::new();
    for i in 1..=5 {
        joined.extend(std::fs::read(key.join(format!("film.mkv.{i:03}"))).unwrap());
    }
    assert_eq!(joined, content);

    // Copy back from the "key": chunks are reassembled automatically.
    let back = t.path().join("back");
    let r = run(&quick_spec(vec![key.clone()], back.clone()));
    assert_eq!(r.status, JobStatus::Completed, "{:?}", r.errors);
    assert_eq!(
        std::fs::read(back.join("key").join("film.mkv")).unwrap(),
        content
    );
    assert!(!back.join("key").join("film.mkv.001").exists());
    assert!(!back.join("key").join("JOIN-film.mkv.cmd").exists());
}

#[test]
fn join_detects_corruption() {
    let t = tempfile::tempdir().unwrap();
    let big = t.path().join("x.iso");
    write(&big, &data(600_000, 9));
    let key = t.path().join("key");
    let mut spec = quick_spec(vec![big], key.clone());
    spec.max_file_size_override = Some(300_000);
    spec.split_chunk_override = Some(262_144);
    run(&spec);
    // Corrupt one byte of part 2.
    let p2 = key.join("x.iso.002");
    let mut b = std::fs::read(&p2).unwrap();
    b[10] ^= 0xFF;
    std::fs::write(&p2, b).unwrap();
    let back = t.path().join("back");
    let r = run(&quick_spec(vec![key.clone()], back.join("b2")));
    assert_eq!(r.status, JobStatus::CompletedWithErrors);
    assert!(r.errors[0].error.contains("corrompu"), "{:?}", r.errors);
    assert!(!back.join("b2").join("key").join("x.iso").exists());
    assert_no_temp(&back);
}

#[test]
fn resume_after_cancel() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    let mut files = vec![];
    for i in 0..4 {
        let d = data(6 * 1_048_576 + i * 1000, i as u8);
        let p = src.join(format!("big{i}.bin"));
        write(&p, &d);
        files.push((p, d));
    }
    for i in 0..50 {
        write(
            &src.join("small").join(format!("{i}.txt")),
            format!("file {i}").as_bytes(),
        );
    }
    let dest = t.path().join("dst");
    let jdir = t.path().join("jobs");
    let mut p = plan::build(&quick_spec(vec![src.clone()], dest.clone())).unwrap();
    p.profile.chunk = 256 * 1024;
    p.profile.depth = 2;
    p.profile.checkpoint_bytes = 1_048_576;
    p.profile.parallel_lanes = false;
    p.profile.small_workers = 2;

    let control = Arc::new(Control::new());
    let c2 = control.clone();
    let fired = AtomicBool::new(false);
    let sink = move |e: Event| {
        if let Event::Progress(s) = e {
            if s.bytes_done > 9 * 1_048_576 && !fired.swap(true, Ordering::SeqCst) {
                c2.cancel();
            }
        }
    };
    let opts = job::ExecOptions {
        journal_dir: Some(jdir.clone()),
        progress_interval: std::time::Duration::from_millis(1),
        ..Default::default()
    };
    // Slow the copy a little so the cancel lands mid-job.
    let r1 = job::execute(&p, &control, &sink, &NonInteractive, opts).unwrap();
    assert_eq!(r1.status, JobStatus::Cancelled);
    let jpath = jdir.join(format!("{}.jsonl", p.id));
    let (_, st) = Journal::load(&jpath).unwrap();
    assert!(
        !st.done.is_empty(),
        "some files were completed before cancel"
    );

    let r2 = job::resume(
        jpath.to_str().unwrap(),
        &Control::new(),
        &NullSink,
        &NonInteractive,
        Default::default(),
    )
    .unwrap();
    assert_eq!(r2.status, JobStatus::Completed, "{:?}", r2.errors);
    for (p, d) in &files {
        let out = dest.join("src").join(p.file_name().unwrap());
        assert_eq!(&std::fs::read(&out).unwrap(), d);
    }
    for i in 0..50 {
        assert_eq!(
            std::fs::read(dest.join("src/small").join(format!("{i}.txt"))).unwrap(),
            format!("file {i}").as_bytes()
        );
    }
    assert_no_temp(&dest);
    assert!(
        !jpath.exists(),
        "journal removed after successful completion"
    );
}

#[test]
fn move_same_volume_and_cross() {
    let t = tempfile::tempdir().unwrap();
    let files = make_tree(t.path());
    let dest = t.path().join("moved");
    let mut spec = quick_spec(vec![t.path().join("src")], dest.clone());
    spec.mode = Mode::Move;
    let p = plan::build(&spec).unwrap();
    assert!(p.same_volume);
    let r = job::execute(
        &p,
        &Control::new(),
        &NullSink,
        &NonInteractive,
        job::ExecOptions {
            journal: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.status, JobStatus::Completed, "{:?}", r.errors);
    for (p, d) in &files {
        assert!(!p.exists());
        let rel = p.strip_prefix(t.path()).unwrap();
        assert_eq!(&std::fs::read(dest.join(rel)).unwrap(), d);
    }
}

#[test]
fn move_with_copy_path_deletes_after_verify() {
    // Force the copy path (not same-volume rename) by clearing the flag.
    let t = tempfile::tempdir().unwrap();
    let files = make_tree(t.path());
    let dest = t.path().join("moved2");
    let mut spec = quick_spec(vec![t.path().join("src")], dest.clone());
    spec.mode = Mode::Move;
    spec.skip_junk = false;
    let mut p = plan::build(&spec).unwrap();
    p.same_volume = false;
    let r = job::execute(
        &p,
        &Control::new(),
        &NullSink,
        &NonInteractive,
        job::ExecOptions {
            journal: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.status, JobStatus::Completed, "{:?}", r.errors);
    for (p, d) in &files {
        assert!(!p.exists(), "source deleted: {}", p.display());
        let rel = p.strip_prefix(t.path()).unwrap();
        assert_eq!(&std::fs::read(dest.join(rel)).unwrap(), d);
    }
    assert!(
        !t.path().join("src").exists(),
        "emptied source dirs removed"
    );
}

#[test]
fn blocks_dest_inside_source() {
    let t = tempfile::tempdir().unwrap();
    write(&t.path().join("src/a.txt"), b"x");
    let p = plan::build(&quick_spec(
        vec![t.path().join("src")],
        t.path().join("src/inner"),
    ))
    .unwrap();
    assert!(!p.blocking_issues().is_empty());
    let e = job::execute(
        &p,
        &Control::new(),
        &NullSink,
        &NonInteractive,
        job::ExecOptions {
            journal: false,
            ..Default::default()
        },
    );
    assert!(matches!(e, Err(Error::Blocked(_))));
}

#[test]
fn many_small_files_parallel() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..2000 {
        write(
            &t.path()
                .join("s")
                .join(format!("d{}", i % 20))
                .join(format!("{i}.txt")),
            &data(i % 3000, i as u8),
        );
    }
    let dest = t.path().join("d");
    let r = run(&quick_spec(vec![t.path().join("s")], dest.clone()));
    assert_eq!(r.status, JobStatus::Completed);
    assert_eq!(r.files_copied, 2000);
    for i in (0..2000).step_by(97) {
        assert_eq!(
            std::fs::read(
                dest.join("s")
                    .join(format!("d{}", i % 20))
                    .join(format!("{i}.txt"))
            )
            .unwrap(),
            data(i % 3000, i as u8)
        );
    }
}
