//! iCalendar (RFC 5545) support for the calendar engine.
//!
//! * [`content`]: a lossless component/property model with parsing and
//!   serialization (folding, escaping, RFC 6868 parameter encoding).
//! * [`values`]: DATE / DATE-TIME / DURATION values and TZID resolution.
//! * [`events`]: VEVENT ⇄ provider-neutral event data, editing objects in
//!   place so unknown properties survive round trips.

pub mod content;
pub mod events;
pub mod values;

pub use content::{Component, Property, parse};
pub use events::{EventData, ObjectEvents, read_events, write_events, write_object};

/// Errors from reading iCalendar data.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IcalError {
    #[error("malformed iCalendar: {0}")]
    Syntax(String),
    #[error("invalid iCalendar value: {0}")]
    Value(String),
}
