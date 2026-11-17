//! # fc-core — the Fast Copy engine
//!
//! Fast, verified, resumable file copies for Windows and macOS.
//!
//! ```no_run
//! use fc_core::*;
//! let spec = JobSpec::new(vec!["C:/data".into()], "E:/backup".into());
//! let plan = plan::build(&spec).unwrap();
//! for issue in &plan.issues { println!("{}", issue.message); }
//! let report = job::execute(&plan, &Control::new(), &NullSink, &NonInteractive, Default::default()).unwrap();
//! println!("{:?}", report.status);
//! ```

pub mod buffer;
pub mod engine;
pub mod error;
pub mod format;
pub mod hash;
pub mod job;
pub mod journal;
pub mod names;
pub mod plan;
pub mod queue;
pub mod shell;
pub mod split;
pub mod sys;
pub mod types;
pub mod util;
pub mod volume;
pub mod vss;

pub use error::{Error, Result};
pub use types::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
