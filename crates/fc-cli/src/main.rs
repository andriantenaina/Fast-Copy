//! `fcopy` — Fast Copy command line.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use clap::{Parser, Subcommand, ValueEnum};
use fc_core::plan::Severity;
use fc_core::util::human_bytes;
use fc_core::*;
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Parser)]
#[command(name = "fcopy", version, about = "Fast Copy — copies rapides, vérifiées et reprenables / fast, verified, resumable copies")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum VerifyArg {
    Fast,
    Full,
    Paranoid,
}

#[derive(Clone, Copy, ValueEnum)]
enum ConflictArg {
    Ask,
    Skip,
    Overwrite,
    Newer,
    KeepBoth,
    IfDifferent,
}

#[derive(clap::Args)]
struct CopyArgs {
    /// Sources puis destination / sources then destination
    #[arg(required = true, num_args = 2..)]
    paths: Vec<PathBuf>,
    /// Déplacer (la source est supprimée après vérification) / move
    #[arg(long = "move", short = 'm')]
    move_: bool,
    /// Niveau de vérification / verification level
    #[arg(long, value_enum, default_value = "full")]
    verify: VerifyArg,
    /// Fichier déjà présent / existing destination file
    #[arg(long, value_enum)]
    on_conflict: Option<ConflictArg>,
    /// Ne jamais découper les fichiers > 4 Go (FAT32) / never split oversize files
    #[arg(long)]
    no_split: bool,
    /// Copier aussi .DS_Store, ._*, Thumbs.db… / also copy OS junk files
    #[arg(long)]
    keep_junk: bool,
    /// Ne pas recoller automatiquement les fichiers découpés / don't rejoin split sets
    #[arg(long)]
    no_join: bool,
    /// Éjecter la destination à la fin / eject destination when done
    #[arg(long)]
    eject: bool,
    /// Copier les fichiers verrouillés via VSS (Windows, admin) / copy locked files via VSS
    #[arg(long)]
    vss: bool,
    /// Nombre de workers pour les petits fichiers / small-file workers
    #[arg(long)]
    threads: Option<usize>,
    /// Sortie JSON (une ligne par événement) / JSON lines output
    #[arg(long)]
    json: bool,
    /// Ignorer les avertissements bloquants (espace…) / run despite blocking issues
    #[arg(long)]
    force: bool,
    /// Pas de journal de reprise / no resume journal
    #[arg(long)]
    no_journal: bool,
    /// Ne rien copier, afficher le plan / only show the plan
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Copier / copy
    #[command(visible_alias = "cp")]
    Copy(CopyArgs),
    /// Déplacer (= copy --move) / move
    #[command(visible_alias = "mv")]
    Move(CopyArgs),
    /// Reprendre une copie interrompue / resume an interrupted job
    Resume {
        /// Identifiant (défaut : la plus récente) / job id (default: most recent)
        id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Lister les copies interrompues / list interrupted jobs
    Jobs {
        /// Abandonner une copie (supprime les fichiers partiels) / discard a job
        #[arg(long)]
        discard: Option<String>,
    },
    /// Comparer source et destination octet par octet (empreintes) / compare trees by hash
    Verify { src: PathBuf, dest: PathBuf },
    /// Découper un fichier (morceaux de 4095 Mio) / split a file
    Split {
        file: PathBuf,
        dest_dir: PathBuf,
        /// Taille des morceaux, ex. 4095M, 2G / chunk size
        #[arg(long)]
        chunk: Option<String>,
    },
    /// Recoller un fichier découpé / rejoin a split file
    Join {
        /// Le manifeste .fcsplit.json ou le premier morceau .001
        input: PathBuf,
        dest_dir: Option<PathBuf>,
    },
    /// Lecteurs : lister, infos, formater en exFAT, éjecter / drives
    Drive {
        #[command(subcommand)]
        cmd: DriveCmd,
    },
    /// Menus clic droit (Explorateur / Finder) / right-click menus
    Shell {
        #[command(subcommand)]
        cmd: ShellCmd,
    },
    /// Mesurer le débit réel vers une destination / measure throughput
    Bench {
        src: PathBuf,
        dest_dir: PathBuf,
        /// Comparer aussi avec la copie système / also time the OS copy
        #[arg(long)]
        compare: bool,
    },
}

#[derive(Subcommand)]
enum DriveCmd {
    /// Lister les lecteurs / list drives
    List,
    /// Détails d'un lecteur / drive details
    Info { path: PathBuf },
    /// Formater en exFAT (EFFACE TOUT) / format to exFAT (ERASES EVERYTHING)
    Format {
        path: PathBuf,
        #[arg(long, default_value = "FASTCOPY")]
        label: String,
        /// 32K, 128K, 1M…
        #[arg(long)]
        cluster: Option<String>,
        /// Ne pas demander confirmation / don't ask
        #[arg(long)]
        yes: bool,
    },
    /// Éjecter proprement / safely eject
    Eject { path: PathBuf },
}

#[derive(Subcommand)]
enum ShellCmd {
    Install {
        /// Chemin de l'application graphique (défaut : à côté de fcopy)
        #[arg(long)]
        app: Option<PathBuf>,
    },
    Uninstall,
    Status,
}

fn main() {
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("erreur : {e:#}");
            2
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> anyhow::Result<i32> {
    match cli.cmd {
        Cmd::Copy(a) => copy(a, false),
        Cmd::Move(a) => copy(a, true),
        Cmd::Resume { id, json } => resume(id, json),
        Cmd::Jobs { discard } => jobs(discard),
        Cmd::Verify { src, dest } => verify_trees(&src, &dest),
        Cmd::Split { file, dest_dir, chunk } => split_cmd(&file, &dest_dir, chunk),
        Cmd::Join { input, dest_dir } => join_cmd(&input, dest_dir),
        Cmd::Drive { cmd } => drive(cmd),
        Cmd::Shell { cmd } => shell(cmd),
        Cmd::Bench { src, dest_dir, compare } => bench(&src, &dest_dir, compare),
    }
}

// ───────────────────────────── copy ─────────────────────────────

fn copy(a: CopyArgs, force_move: bool) -> anyhow::Result<i32> {
    let mut paths = a.paths;
    let dest = paths.pop().expect("clap ensures ≥ 2");
    let mut spec = JobSpec::new(paths, dest);
    spec.mode = if a.move_ || force_move { Mode::Move } else { Mode::Copy };
    spec.verify = match a.verify {
        VerifyArg::Fast => VerifyMode::Fast,
        VerifyArg::Full => VerifyMode::Full,
        VerifyArg::Paranoid => VerifyMode::Paranoid,
    };
    let interactive = std::io::stdin().is_terminal() && !a.json;
    spec.on_conflict = match a.on_conflict {
        Some(c) => conflict(c),
        None if interactive => OnConflict::Ask,
        None => OnConflict::Skip,
    };
    spec.split = if a.no_split { SplitMode::Never } else { SplitMode::Auto };
    spec.skip_junk = !a.keep_junk;
    spec.auto_join = !a.no_join;
    spec.use_vss = a.vss;
    spec.threads = a.threads;

    let plan = fc_core::plan::build(&spec)?;
    if !a.json {
        print_plan(&plan);
    } else {
        println!("{}", serde_json::json!({"type":"plan","id":plan.id,"files":plan.total_files,"bytes":plan.total_bytes,"issues":plan.issues,"profile":plan.profile.name}));
    }
    if a.dry_run {
        return Ok(if plan.blocking_issues().is_empty() { 0 } else { 2 });
    }
    if !plan.blocking_issues().is_empty() && !a.force {
        if !a.json {
            eprintln!("\nCopie bloquée. Corrigez les problèmes ci-dessus, ou relancez avec --force si vous êtes sûr.");
            if plan.issues.iter().any(|i| matches!(i.kind, fc_core::plan::IssueKind::FileTooLarge { .. } | fc_core::plan::IssueKind::ReadOnlyDestination { .. })) {
                eprintln!("Astuce : `fcopy drive format {} --label MACLE` formate la clé en exFAT (lisible et inscriptible sur Windows et Mac).", plan.dst_volume.root.display());
            }
        }
        return Ok(2);
    }
    let opts = job::ExecOptions { journal: !a.no_journal, force: a.force, ..Default::default() };
    let report = execute_with_ui(|control, sink, ui| job::execute(&plan, control, sink, ui, opts), a.json, interactive)?;
    let code = finish(&report, a.json);
    if a.eject && report.status != JobStatus::Cancelled {
        if !a.json {
            println!("Éjection de {}…", plan.dst_volume.root.display());
        }
        match volume::eject(&plan.dst_volume.root) {
            Ok(()) => {
                if !a.json {
                    println!("✓ Vous pouvez retirer le support en toute sécurité.");
                }
            }
            Err(e) => eprintln!("éjection impossible : {e}"),
        }
    }
    Ok(code)
}

fn conflict(c: ConflictArg) -> OnConflict {
    match c {
        ConflictArg::Ask => OnConflict::Ask,
        ConflictArg::Skip => OnConflict::Skip,
        ConflictArg::Overwrite => OnConflict::Overwrite,
        ConflictArg::Newer => OnConflict::Newer,
        ConflictArg::KeepBoth => OnConflict::KeepBoth,
        ConflictArg::IfDifferent => OnConflict::IfDifferent,
    }
}

fn print_plan(p: &fc_core::plan::Plan) {
    println!(
        "Plan : {} fichier(s), {} dossier(s), {}  →  {} ({}, {})",
        p.total_files,
        p.total_dirs,
        human_bytes(p.total_bytes),
        p.spec.dest.display(),
        p.dst_volume.fs.label(),
        p.profile.name
    );
    if p.dst_volume.free_bytes != u64::MAX {
        println!("Espace libre : {}", human_bytes(p.dst_volume.free_bytes));
    }
    for i in &p.issues {
        let tag = match i.severity {
            Severity::Error => "✗ ERREUR",
            Severity::Warning => "! attention",
            Severity::Info => "i info",
        };
        println!("  {tag} : {}", i.message);
    }
    for (from, to) in p.renamed.iter().take(10) {
        println!("    renommé : {from}  →  {to}");
    }
    if p.renamed.len() > 10 {
        println!("    … et {} autre(s)", p.renamed.len() - 10);
    }
}

/// Terminal interaction: conflict and disk-full prompts.
struct CliUi {
    bar: Arc<Mutex<Option<ProgressBar>>>,
    interactive: bool,
}

fn prompt(bar: &Arc<Mutex<Option<ProgressBar>>>, text: &str) -> String {
    let ask = || {
        eprint!("{text}");
        let _ = std::io::stderr().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        s.trim().to_string()
    };
    match bar.lock().unwrap().as_ref() {
        Some(b) => b.suspend(ask),
        None => ask(),
    }
}

impl Interaction for CliUi {
    fn resolve_conflict(&self, i: &ConflictInfo) -> ConflictDecision {
        if !self.interactive {
            return ConflictDecision { action: OnConflict::Skip, apply_to_all: true };
        }
        let newer = if i.src_mtime_ms > i.dst_mtime_ms { "source plus récente" } else { "destination plus récente ou identique" };
        let text = format!(
            "\n« {} » existe déjà.\n  source : {}  |  destination : {}  ({newer})\n  [r]emplacer  [i]gnorer  [g]arder les deux  [p]lus récent  [d]ifférent — MAJUSCULE = pour tous : ",
            i.path,
            human_bytes(i.src_size),
            human_bytes(i.dst_size)
        );
        loop {
            let ans = prompt(&self.bar, &text);
            // Upper-case letter (or a trailing '!') = apply to all remaining conflicts.
            let all = ans.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) || ans.ends_with('!');
            let action = match ans.to_lowercase().trim_end_matches('!').chars().next() {
                Some('r') => OnConflict::Overwrite,
                Some('i') => OnConflict::Skip,
                Some('g') => OnConflict::KeepBoth,
                Some('p') => OnConflict::Newer,
                Some('d') => OnConflict::IfDifferent,
                _ => continue,
            };
            return ConflictDecision { action, apply_to_all: all };
        }
    }

    fn disk_full(&self, path: &str, needed: u64, available: u64) -> DiskFullAction {
        if !self.interactive {
            return DiskFullAction::Abort;
        }
        let text = format!(
            "\nDisque plein en copiant « {path} » ({} nécessaires, {} libres). Libérez de la place puis : [r]éessayer  [i]gnorer ce fichier  [a]nnuler : ",
            human_bytes(needed),
            human_bytes(available)
        );
        loop {
            match prompt(&self.bar, &text).to_lowercase().chars().next() {
                Some('r') => return DiskFullAction::Retry,
                Some('i') => return DiskFullAction::Skip,
                Some('a') => return DiskFullAction::Abort,
                _ => {}
            }
        }
    }
}

fn execute_with_ui(
    f: impl FnOnce(&Control, &dyn EventSink, &dyn Interaction) -> fc_core::Result<Report>,
    json: bool,
    interactive: bool,
) -> anyhow::Result<Report> {
    let control = Arc::new(Control::new());
    {
        let c = control.clone();
        let _ = ctrlc::set_handler(move || {
            if c.is_cancelled() {
                std::process::exit(130);
            }
            eprintln!("\nInterruption : arrêt propre en cours… (Ctrl+C à nouveau pour forcer). Reprise possible avec `fcopy resume`.");
            c.cancel();
        });
    }
    let bar_slot: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
    if !json {
        let pb = ProgressBar::new(0);
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} [{elapsed_precise}] {wide_bar:.cyan/blue} {bytes}/{total_bytes} {msg}")
                .unwrap()
                .progress_chars("━╸─"),
        );
        pb.enable_steady_tick(Duration::from_millis(120));
        *bar_slot.lock().unwrap() = Some(pb);
    }
    let bar2 = bar_slot.clone();
    let sink = move |e: Event| {
        if json {
            if let Ok(s) = serde_json::to_string(&e) {
                println!("{s}");
            }
            return;
        }
        let guard = bar2.lock().unwrap();
        let Some(pb) = guard.as_ref() else { return };
        match e {
            Event::Planned { bytes, .. } => pb.set_length(bytes),
            Event::Progress(s) => {
                pb.set_length(s.bytes_total);
                pb.set_position(s.bytes_done);
                let eta = s.eta_secs.map(|e| format!(" · {}", fmt_dur(e))).unwrap_or_default();
                let pause = if s.paused { " ⏸" } else { "" };
                pb.set_message(format!(
                    "{}/s{eta} · {}/{} fichiers{pause} · {}",
                    human_bytes(s.speed as u64),
                    s.files_done + s.files_skipped,
                    s.files_total,
                    truncate_middle(&s.current, 40)
                ));
            }
            Event::FileFailed { path, error } => pb.println(format!("✗ {path} : {error}")),
            _ => {}
        }
    };
    let ui = CliUi { bar: bar_slot.clone(), interactive };
    let r = f(&control, &sink, &ui);
    if let Some(pb) = bar_slot.lock().unwrap().take() {
        pb.finish_and_clear();
    }
    Ok(r?)
}

fn finish(r: &Report, json: bool) -> i32 {
    if !json {
        let speed = human_bytes(r.avg_speed as u64);
        match r.status {
            JobStatus::Completed => println!(
                "✓ Terminé : {} fichier(s), {} en {} ({speed}/s){}",
                r.files_copied,
                human_bytes(r.bytes_copied),
                fmt_dur(r.elapsed_ms / 1000),
                if r.files_skipped > 0 { format!(", {} ignoré(s)", r.files_skipped) } else { String::new() }
            ),
            JobStatus::CompletedWithErrors => println!(
                "! Terminé avec {} erreur(s) : {} fichier(s) copié(s), {} ignoré(s)",
                r.files_failed, r.files_copied, r.files_skipped
            ),
            JobStatus::Cancelled => println!("■ Annulé. Reprenez avec : fcopy resume {}", r.job_id),
            JobStatus::Failed => println!("✗ Échec."),
        }
        for e in r.errors.iter().take(20) {
            println!("  ✗ {} : {}", e.path, e.error);
        }
        for w in r.warnings.iter().take(10) {
            println!("  ! {w}");
        }
        if r.warnings.len() > 10 {
            println!("  … {} autre(s) avertissement(s)", r.warnings.len() - 10);
        }
        if !r.locked.is_empty() {
            println!("  {} fichier(s) verrouillé(s) : relancez avec --vss pour les copier via un cliché instantané.", r.locked.len());
        }
        if r.status == JobStatus::CompletedWithErrors {
            println!("  Les fichiers en erreur peuvent être retentés avec : fcopy resume {}", r.job_id);
        }
    }
    match r.status {
        JobStatus::Completed => 0,
        JobStatus::CompletedWithErrors => 1,
        JobStatus::Cancelled => 130,
        JobStatus::Failed => 2,
    }
}

fn fmt_dur(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn truncate_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let half = (max - 1) / 2;
    let start: String = s.chars().take(half).collect();
    let end: String = s.chars().skip(n - half).collect();
    format!("{start}…{end}")
}

// ───────────────────────────── resume / jobs ─────────────────────────────

fn resume(id: Option<String>, json: bool) -> anyhow::Result<i32> {
    let id = match id {
        Some(i) => i,
        None => journal::list_jobs()
            .into_iter()
            .find(|j| j.resumable())
            .map(|j| j.id)
            .ok_or_else(|| anyhow!("aucune copie interrompue"))?,
    };
    if !json {
        println!("Reprise de la copie {id}…");
    }
    let interactive = std::io::stdin().is_terminal() && !json;
    let report = execute_with_ui(|c, s, u| job::resume(&id, c, s, u, Default::default()), json, interactive)?;
    Ok(finish(&report, json))
}

fn jobs(discard: Option<String>) -> anyhow::Result<i32> {
    if let Some(id) = discard {
        let n = journal::discard_job(&id)?;
        println!("Copie {id} abandonnée ({n} fichier(s) partiel(s) supprimé(s)).");
        return Ok(0);
    }
    let list = journal::list_jobs();
    if list.is_empty() {
        println!("Aucune copie interrompue.");
    }
    for j in list {
        println!(
            "{}  {}/{} fichiers  {}  {} → {}  [{}]",
            j.id,
            j.done_files,
            j.total_files,
            human_bytes(j.total_bytes),
            j.sources.iter().map(|s| s.display().to_string()).collect::<Vec<_>>().join(", "),
            j.dest.display(),
            j.ended.as_deref().unwrap_or("interrompue")
        );
    }
    Ok(0)
}

// ───────────────────────────── verify ─────────────────────────────

fn verify_trees(src: &Path, dest: &Path) -> anyhow::Result<i32> {
    let profile = fc_core::plan::Profile::choose(&volume::detect(src), &volume::detect(dest), None);
    let control = Control::new();
    let stats = Stats::default();
    let warnings = Mutex::new(vec![]);
    let dst_fs = volume::detect(dest).fs;
    let ctx = engine::Ctx { profile: &profile, control: &control, stats: &stats, verify: VerifyMode::Full, dst_fs: &dst_fs, warnings: &warnings };
    let mut checked = 0u64;
    let mut bad = 0u64;
    let base = dest.join(src.file_name().unwrap_or_default());
    let files: Vec<(PathBuf, PathBuf)> = if src.is_dir() {
        let mut v = vec![];
        for e in walk(src) {
            let rel = e.strip_prefix(src).unwrap().to_path_buf();
            v.push((e, base.join(rel)));
        }
        v
    } else {
        vec![(src.to_path_buf(), base.clone())]
    };
    for (s, d) in files {
        checked += 1;
        let a = engine::fingerprint_file(&ctx, &s);
        let b = engine::fingerprint_file(&ctx, &d);
        match (a, b) {
            (Ok(x), Ok(y)) if x == y => {}
            (Ok(_), Ok(_)) => {
                bad += 1;
                println!("✗ différent : {}", d.display());
            }
            (_, Err(e)) => {
                bad += 1;
                println!("✗ {} : {e}", d.display());
            }
            (Err(e), _) => {
                bad += 1;
                println!("✗ {} : {e}", s.display());
            }
        }
    }
    if bad == 0 {
        println!("✓ {checked} fichier(s) identiques.");
        Ok(0)
    } else {
        println!("✗ {bad} différence(s) sur {checked} fichier(s).");
        Ok(1)
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                out.extend(walk(&p));
            } else if ft.is_file() && !names::is_junk(&e.file_name().to_string_lossy()) {
                out.push(p);
            }
        }
    }
    out
}

// ───────────────────────────── split / join ─────────────────────────────

fn parse_size(s: &str) -> anyhow::Result<u64> {
    let s = s.trim().to_uppercase();
    let (num, mult) = match s.chars().last() {
        Some('K') => (&s[..s.len() - 1], 1024u64),
        Some('M') => (&s[..s.len() - 1], 1024 * 1024),
        Some('G') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (&s[..], 1),
    };
    let v: f64 = num.parse().with_context(|| format!("taille invalide « {s} »"))?;
    Ok((v * mult as f64) as u64)
}

fn split_cmd(file: &Path, dest_dir: &Path, chunk: Option<String>) -> anyhow::Result<i32> {
    let chunk = match chunk {
        Some(c) => parse_size(&c)?,
        None => split::SPLIT_CHUNK,
    };
    if chunk < 4096 || chunk % 4096 != 0 {
        bail!("la taille des morceaux doit être un multiple de 4 Ko");
    }
    let mut spec = JobSpec::new(vec![file.to_path_buf()], dest_dir.to_path_buf());
    spec.max_file_size_override = Some(chunk.min(std::fs::metadata(file)?.len().saturating_sub(1)));
    spec.split_chunk_override = Some(chunk);
    spec.on_conflict = OnConflict::Overwrite;
    let plan = fc_core::plan::build(&spec)?;
    let report = execute_with_ui(|c, s, u| job::execute(&plan, c, s, u, job::ExecOptions { journal: false, ..Default::default() }), false, false)?;
    Ok(finish(&report, false))
}

fn join_cmd(input: &Path, dest_dir: Option<PathBuf>) -> anyhow::Result<i32> {
    let manifest = if input.to_string_lossy().ends_with(split::MANIFEST_SUFFIX) {
        input.to_path_buf()
    } else {
        let s = input.to_string_lossy();
        let base = s.rsplit_once('.').map(|(b, _)| b.to_string()).unwrap_or_else(|| s.to_string());
        PathBuf::from(format!("{base}{}", split::MANIFEST_SUFFIX))
    };
    let m = split::SplitManifest::load(&manifest)?;
    let dir = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
    let dest_dir = dest_dir.unwrap_or_else(|| dir.clone());
    let parts: Vec<PathBuf> = m.parts.iter().map(|p| dir.join(&p.name)).collect();
    let final_path = dest_dir.join(&m.original_name);
    let dvol = volume::detect(&dest_dir);
    if m.total_size > dvol.fs.max_file_size() {
        bail!("la destination ({}) ne peut pas contenir un fichier de {}", dvol.fs.label(), human_bytes(m.total_size));
    }
    std::fs::create_dir_all(&dest_dir)?;
    let profile = fc_core::plan::Profile::choose(&volume::detect(&dir), &dvol, None);
    let control = Control::new();
    let stats = Stats::default();
    let warnings = Mutex::new(vec![]);
    let ctx = engine::Ctx { profile: &profile, control: &control, stats: &stats, verify: VerifyMode::Full, dst_fs: &dvol.fs, warnings: &warnings };
    println!("Recollage de {} ({} morceaux, {})…", m.original_name, parts.len(), human_bytes(m.total_size));
    let w = engine::write_join(&ctx, &manifest, &parts, &final_path)?;
    engine::commit(&w.tmp, &final_path, false)?;
    println!("✓ {} recollé et vérifié (SHA-256 {}).", final_path.display(), m.sha256);
    Ok(0)
}

// ───────────────────────────── drives ─────────────────────────────

fn drive(cmd: DriveCmd) -> anyhow::Result<i32> {
    match cmd {
        DriveCmd::List => {
            for v in volume::list_volumes() {
                println!(
                    "{:<14} {:<7} {:>10} libres / {:>10}  {:<6} {}{}{}",
                    v.root.display(),
                    v.fs.label(),
                    human_bytes(v.free_bytes),
                    human_bytes(v.total_bytes),
                    format!("{:?}", v.bus).to_lowercase(),
                    if v.label.is_empty() { String::new() } else { format!("« {} » ", v.label) },
                    v.model,
                    if v.is_system { "  [système]" } else { "" }
                );
            }
            Ok(0)
        }
        DriveCmd::Info { path } => {
            let v = volume::volume_info(&path)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
            if v.fs == volume::FsKind::Fat32 {
                println!("\nFAT32 : fichiers limités à 4 Go. `fcopy drive format {} --label NOM` passe en exFAT (Windows + Mac, sans limite).", v.root.display());
            }
            if v.fs == volume::FsKind::Ntfs {
                println!("\nNTFS : macOS ne peut que lire ce disque. Formatez en exFAT pour écrire depuis un Mac.");
            }
            Ok(0)
        }
        DriveCmd::Format { path, label, cluster, yes } => {
            let v = volume::volume_info(&path)?;
            format::check_formattable(&v)?;
            let cluster = cluster.map(|c| parse_size(&c)).transpose()?;
            println!(
                "Formatage en exFAT de {} « {} » — {} {} ({}, bus {:?}).",
                v.root.display(),
                v.label,
                v.model,
                human_bytes(v.total_bytes),
                v.fs.label(),
                v.bus
            );
            println!("⚠ TOUTES LES DONNÉES DE CE SUPPORT SERONT DÉFINITIVEMENT EFFACÉES.");
            if !yes {
                let expected = if v.label.is_empty() { "EFFACER".to_string() } else { v.label.clone() };
                eprint!("Tapez « {expected} » pour confirmer : ");
                let _ = std::io::stderr().flush();
                let mut s = String::new();
                std::io::stdin().read_line(&mut s)?;
                if s.trim() != expected {
                    println!("Annulé.");
                    return Ok(1);
                }
            }
            let nv = format::format_exfat(&format::FormatRequest { volume: path, label, cluster })?;
            println!("✓ Formaté : {} ({}, {} libres).", nv.root.display(), nv.fs.label(), human_bytes(nv.free_bytes));
            Ok(0)
        }
        DriveCmd::Eject { path } => {
            volume::eject(&path)?;
            println!("✓ Vous pouvez retirer le support en toute sécurité.");
            Ok(0)
        }
    }
}

// ───────────────────────────── shell ─────────────────────────────

fn shell(cmd: ShellCmd) -> anyhow::Result<i32> {
    match cmd {
        ShellCmd::Install { app } => {
            let app = app.unwrap_or_else(|| {
                let exe = std::env::current_exe().unwrap_or_default();
                exe.with_file_name(if cfg!(windows) { "fast-copy.exe" } else { "fast-copy" })
            });
            if !app.exists() {
                bail!("application introuvable : {} (utilisez --app)", app.display());
            }
            shell::install(&app, &shell::system_lang())?;
            println!("✓ Menus clic droit installés pour {}.", app.display());
            Ok(0)
        }
        ShellCmd::Uninstall => {
            shell::uninstall()?;
            println!("✓ Menus clic droit retirés.");
            Ok(0)
        }
        ShellCmd::Status => {
            println!("{}", if shell::is_installed() { "installé" } else { "non installé" });
            Ok(0)
        }
    }
}

// ───────────────────────────── bench ─────────────────────────────

fn bench(src: &Path, dest_dir: &Path, compare: bool) -> anyhow::Result<i32> {
    let work = dest_dir.join(format!(".fastcopy-bench-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let result = (|| -> anyhow::Result<()> {
        for verify in [VerifyMode::Fast, VerifyMode::Full] {
            let out = work.join(format!("{verify:?}"));
            let mut spec = JobSpec::new(vec![src.to_path_buf()], out);
            spec.verify = verify;
            spec.on_conflict = OnConflict::Overwrite;
            let plan = fc_core::plan::build(&spec)?;
            let r = job::execute(&plan, &Control::new(), &NullSink, &NonInteractive, job::ExecOptions { journal: false, force: true, ..Default::default() })?;
            println!(
                "Fast Copy (vérification {:<8}) : {} en {:.2} s → {}/s  [profil {}]",
                format!("{verify:?}").to_lowercase(),
                human_bytes(r.bytes_copied),
                r.elapsed_ms as f64 / 1000.0,
                human_bytes(r.avg_speed as u64),
                plan.profile.name
            );
        }
        if compare && src.is_file() {
            let out = work.join("os-copy");
            std::fs::create_dir_all(&out)?;
            let t = std::time::Instant::now();
            let n = std::fs::copy(src, out.join(src.file_name().unwrap()))?;
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(out.join(src.file_name().unwrap())) {
                let _ = f.sync_all();
            }
            let s = t.elapsed().as_secs_f64();
            println!("Copie système (sans vérification) : {} en {s:.2} s → {}/s", human_bytes(n), human_bytes((n as f64 / s) as u64));
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result.map(|_| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sizes() {
        assert_eq!(parse_size("128K").unwrap(), 131072);
        assert_eq!(parse_size("1M").unwrap(), 1 << 20);
        assert_eq!(parse_size("4095m").unwrap(), split::SPLIT_CHUNK);
    }
    #[test]
    fn trunc() {
        assert_eq!(truncate_middle("abcdefghij", 5), "ab…ij");
        assert_eq!(truncate_middle("abc", 5), "abc");
    }
}
