//! Domain contracts for the local-first file organizer.
//!
//! This crate intentionally has no dependency on Tauri, SQLite, an AI vendor,
//! or an operating-system API.  Adapters may depend on the domain; the domain
//! never depends on adapters.

mod ai;
mod execution;
mod file;
mod ids;
mod operation;
mod proposal;
mod rules;
mod search;

pub use ai::*;
pub use execution::*;
pub use file::*;
pub use ids::*;
pub use operation::*;
pub use proposal::*;
pub use rules::*;
pub use search::*;
