//! The few strings shown by the Rust side (tray menu, notifications).

pub struct Texts {
    pub open: &'static str,
    pub quit: &'static str,
    pub tooltip: &'static str,
}

pub fn texts(lang: &str) -> Texts {
    if lang == "fr" {
        Texts {
            open: "Ouvrir Fast Copy",
            quit: "Quitter",
            tooltip: "Fast Copy",
        }
    } else {
        Texts {
            open: "Open Fast Copy",
            quit: "Quit",
            tooltip: "Fast Copy",
        }
    }
}

pub fn items_ready(lang: &str, n: usize, moving: bool) -> (String, String) {
    if lang == "fr" {
        let what = if moving { "à déplacer" } else { "à copier" };
        (
            format!("{n} élément(s) prêt(s) {what}"),
            "Clic droit › Coller ici avec Fast Copy".into(),
        )
    } else {
        let what = if moving { "to move" } else { "to copy" };
        (
            format!("{n} item(s) ready {what}"),
            "Right-click › Paste here with Fast Copy".into(),
        )
    }
}

/// French-style or English-style human size.
pub fn bytes(lang: &str, b: u64) -> String {
    let units: [&str; 5] = if lang == "fr" {
        ["o", "Ko", "Mo", "Go", "To"]
    } else {
        ["B", "KB", "MB", "GB", "TB"]
    };
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let s = if i == 0 {
        format!("{}", b)
    } else if v < 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    };
    let s = if lang == "fr" { s.replace('.', ",") } else { s };
    format!("{s} {}", units[i])
}

fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{:02}", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}min {:02}s", s / 60, s % 60)
    } else {
        format!("{}s", s.max(1))
    }
}

pub fn job_finished(lang: &str, r: &fc_core::Report) -> (String, String) {
    use fc_core::JobStatus::*;
    let fr = lang == "fr";
    let title = match (r.status, fr) {
        (Completed, true) => "Copie terminée",
        (Completed, false) => "Copy complete",
        (CompletedWithErrors, true) => "Copie terminée avec des erreurs",
        (CompletedWithErrors, false) => "Copy finished with errors",
        (Cancelled, true) => "Copie annulée",
        (Cancelled, false) => "Copy cancelled",
        (Failed, true) => "Échec de la copie",
        (Failed, false) => "Copy failed",
    };
    let speed = bytes(lang, r.avg_speed as u64);
    let body = if fr {
        let mut b = format!(
            "{} fichier(s), {} en {} ({}/s)",
            r.files_copied,
            bytes(lang, r.bytes_copied),
            duration(r.elapsed_ms),
            speed
        );
        if r.files_failed > 0 {
            b.push_str(&format!(" — {} erreur(s)", r.files_failed));
        }
        b
    } else {
        let mut b = format!(
            "{} file(s), {} in {} ({}/s)",
            r.files_copied,
            bytes(lang, r.bytes_copied),
            duration(r.elapsed_ms),
            speed
        );
        if r.files_failed > 0 {
            b.push_str(&format!(" — {} error(s)", r.files_failed));
        }
        b
    };
    (title.into(), body)
}
