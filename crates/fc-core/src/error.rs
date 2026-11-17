use std::io;
use std::path::PathBuf;

/// Errors produced by the Fast Copy engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{op} « {path} » : {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("vérification échouée pour « {0} » : le contenu écrit ne correspond pas à la source")]
    VerifyMismatch(PathBuf),
    #[error("la source « {0} » a été modifiée pendant la copie")]
    SourceChanged(PathBuf),
    #[error("copie annulée")]
    Cancelled,
    #[error("espace insuffisant sur la destination : {needed} octets nécessaires, {available} disponibles")]
    InsufficientSpace { needed: u64, available: u64 },
    #[error("plan bloqué : {0}")]
    Blocked(String),
    #[error("journal : {0}")]
    Journal(String),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(op: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Error::Io {
            op,
            path: path.into(),
            source,
        }
    }

    /// True if the underlying OS error means "disk full".
    pub fn is_disk_full(&self) -> bool {
        match self {
            Error::Io { source, .. } => is_disk_full(source),
            Error::InsufficientSpace { .. } => true,
            _ => false,
        }
    }

    /// True if the error is a sharing violation / locked file (Windows).
    pub fn is_locked(&self) -> bool {
        match self {
            Error::Io { source, .. } => is_locked(source),
            _ => false,
        }
    }
}

pub fn is_disk_full(e: &io::Error) -> bool {
    match e.raw_os_error() {
        #[cfg(windows)]
        Some(39) | Some(112) => true, // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
        #[cfg(unix)]
        Some(code) if code == libc::ENOSPC || code == libc::EDQUOT => true,
        _ => e.kind() == io::ErrorKind::StorageFull,
    }
}

pub fn is_locked(e: &io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(e.raw_os_error(), Some(32) | Some(33)) // SHARING_VIOLATION, LOCK_VIOLATION
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Helper to attach path context to io results.
pub trait IoContext<T> {
    fn ctx(self, op: &'static str, path: &std::path::Path) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn ctx(self, op: &'static str, path: &std::path::Path) -> Result<T> {
        self.map_err(|e| Error::io(op, path, e))
    }
}
