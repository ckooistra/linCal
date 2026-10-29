use thiserror::Error;

/// Errors from validating or expanding calendar data.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("the event must end after it starts")]
    EndBeforeStart,
    #[error("unknown time zone: {0}")]
    UnknownTimeZone(String),
    #[error("invalid recurrence rule: {0}")]
    InvalidRecurrence(String),
    #[error("invalid occurrence id: {0}")]
    InvalidOccurrenceKey(String),
    #[error("the title is too long")]
    TitleTooLong,
    #[error("invalid time value")]
    InvalidTime,
    #[error("invalid date: {0}")]
    InvalidDate(String),
}
