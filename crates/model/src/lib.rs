//! Pure logic over snapshots: no Windows calls, fully unit-testable.
pub mod rates;

pub use rates::{ProcRates, RateTracker};
