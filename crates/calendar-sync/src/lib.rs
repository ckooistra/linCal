//! Background synchronization between local storage and calendar providers.
//!
//! * Local changes are already durable in SQLite with a queued operation
//!   (see `calendar-storage`); this crate uploads them with version checks,
//!   retries transient failures with exponential backoff and records
//!   conflicts instead of overwriting server changes.
//! * Server changes are downloaded incrementally (sync tokens) where the
//!   provider supports it and applied in one transaction per calendar.
//! * Accounts are isolated: one failing, slow or panicking account does not
//!   stop the others, and a failing account backs off instead of retrying
//!   in a tight loop.
//! * Nothing here blocks the UI; passes run on Tokio and report progress
//!   through a listener.

pub mod credentials;
pub mod engine;
pub mod scheduler;

pub use calendar_oauth::OAuthError;
pub use calendar_storage::Resolution;
#[cfg(feature = "keyring")]
pub use credentials::KeyringCredentials;
pub use credentials::{CredentialStore, MemoryCredentials};
pub use engine::{
    BrowserOpener, CalDavConfig, OAuthConfig, SyncEngine, SyncError, SyncEvent, SyncProblem,
    SyncReport, backoff, codec_for, default_factory, user_message,
};
pub use scheduler::{SchedulerHandle, scheduler};
/// Signal used to cancel a long operation (account setup).
pub use tokio::sync::Notify as CancelSignal;
