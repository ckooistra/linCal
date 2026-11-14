//! Keeps two programs that share a database (the desktop app and `ucal`)
//! from syncing the same account at the same time. The engine's own locks
//! only see one process; this adds an OS file lock per account. The OS
//! releases it when the file is closed, including when a program crashes.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// The lock file for `account_id` in `dir`. Ids are made of letters,
/// digits and dashes; anything else is replaced to keep a plain file name.
fn lock_path(dir: &Path, account_id: &str) -> PathBuf {
    let name: String = account_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("sync-{name}.lock"))
}

/// Waits until no other process syncs `account_id`, then holds the lock
/// until the returned file is dropped. The wait runs on the blocking pool;
/// if the caller gives up, the lock is released as soon as it is taken.
pub(crate) async fn lock_account(dir: &Path, account_id: &str) -> io::Result<File> {
    let path = lock_path(dir, account_id);
    tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        file.lock()?;
        Ok(file)
    })
    .await
    .map_err(io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lock_files_have_plain_names() {
        let dir = Path::new("/data");
        assert_eq!(
            lock_path(dir, "caldav-18f3a-0"),
            dir.join("sync-caldav-18f3a-0.lock")
        );
        assert_eq!(lock_path(dir, "../x y"), dir.join("sync-___x_y.lock"));
    }

    #[tokio::test]
    async fn a_second_holder_waits_for_the_first() {
        let dir = std::env::temp_dir().join(format!("uc-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Another program holds the lock (a separate open file behaves
        // like another process).
        let other = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(lock_path(&dir, "acc"))
            .unwrap();
        other.lock().unwrap();

        let waiting = tokio::spawn({
            let dir = dir.clone();
            async move { lock_account(&dir, "acc").await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "must wait while the lock is held");
        // Other accounts are not affected.
        drop(lock_account(&dir, "other").await.unwrap());

        drop(other);
        let held = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("lock taken once released")
            .unwrap();
        assert!(held.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
