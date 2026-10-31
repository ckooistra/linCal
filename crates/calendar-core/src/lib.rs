//! Provider-neutral calendar engine: models, validation and recurrence.
//!
//! This crate has no knowledge of Tauri, SQLite or any provider. It is used
//! by `calendar-storage` today and by sync and provider crates later.

pub mod error;
pub mod merge;
pub mod model;
pub mod occurrence_key;
pub mod recurrence;
#[cfg(feature = "sim")]
pub mod sim;
pub mod timing;
pub mod wire;

pub use error::CoreError;
pub use model::*;
pub use occurrence_key::OccurrenceKey;
pub use timing::{TimeRange, parse_time_zone};
