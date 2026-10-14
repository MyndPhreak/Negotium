//! Windows data sources. The only crate besides `negotium-bench` that calls Windows APIs.
pub mod processes;

pub use processes::{ProcessReader, RawProcess};
