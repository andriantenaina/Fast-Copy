//! User preferences, stored as JSON in `<config>/FastCopy/settings.json`.

use fc_core::{OnConflict, VerifyMode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "auto", "fr" or "en".
    pub lang: String,
    pub verify: VerifyMode,
    pub on_conflict: OnConflict,
    pub skip_junk: bool,
    /// Stream per-file events to the UI (recent files ticker).
    pub file_events: bool,
    /// Notify when a job finishes.
    pub notify: bool,
    /// Install the context menus automatically at startup if they are missing (MSI installs
    /// do not run the NSIS hook). Turned off when the user removes them in Settings.
    pub auto_shell: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { lang: "auto".into(), verify: VerifyMode::Full, on_conflict: OnConflict::Ask, skip_junk: true, file_events: true, notify: true, auto_shell: true }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("FastCopy").join("settings.json")
    }

    pub fn load() -> Settings {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    /// Effective UI language ("fr" or "en").
    pub fn resolved_lang(&self) -> String {
        match self.lang.as_str() {
            "fr" | "en" => self.lang.clone(),
            _ => {
                if fc_core::shell::system_lang().starts_with("fr") {
                    "fr".into()
                } else {
                    "en".into()
                }
            }
        }
    }
}
