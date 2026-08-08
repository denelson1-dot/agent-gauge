//! Single-instance enforcement.
//!
//! Both platforms use a lock file in the state directory, but they establish
//! ownership differently because the operating systems offer different
//! guarantees.
//!
//! Linux has no mandatory file locking, so the file records a PID and liveness
//! is checked through `/proc`. Windows can do better: opening the file with a
//! share mode of zero means the OS itself refuses a second open for as long as
//! the first process holds the handle. That is stronger than a PID check — it
//! cannot be fooled by PID reuse — and it self-heals after a crash, because a
//! file with no live holder opens normally.

use std::{fs, path::PathBuf};

use crate::platform::dirs;

const LOCK_FILE: &str = "agent-gauge.lock";

/// Held for the lifetime of the process. Dropping it releases the lock.
pub struct Guard {
    path: PathBuf,
    _file: fs::File,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn acquire() -> Result<Guard, String> {
    let path = dirs::state_dir().join(LOCK_FILE);
    dirs::ensure_parent(&path)?;
    acquire_at(path)
}

#[cfg(target_os = "linux")]
fn acquire_at(path: PathBuf) -> Result<Guard, String> {
    use std::{fs::OpenOptions, io::Write};

    fn create(path: &PathBuf) -> std::io::Result<fs::File> {
        let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;
        Ok(file)
    }

    match create(&path) {
        Ok(file) => Ok(Guard { path, _file: file }),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let pid = fs::read_to_string(&path)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok());
            if pid.is_some_and(process_exists) {
                return Err("another instance is active".into());
            }
            fs::remove_file(&path)
                .map_err(|remove| format!("could not remove stale lock: {remove}"))?;
            create(&path)
                .map(|file| Guard { path, _file: file })
                .map_err(|retry| format!("could not create instance lock: {retry}"))
        }
        Err(error) => Err(format!("could not create instance lock: {error}")),
    }
}

#[cfg(target_os = "linux")]
fn process_exists(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

#[cfg(target_os = "windows")]
fn acquire_at(path: PathBuf) -> Result<Guard, String> {
    use std::{fs::OpenOptions, io::Write, os::windows::fs::OpenOptionsExt};

    // share_mode(0) asks Windows for exclusive access. While this handle is
    // open no other process can open the file at all, which is what makes the
    // lock authoritative rather than advisory.
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .share_mode(0)
        .open(&path);

    match file {
        Ok(mut file) => {
            // Recorded for diagnostics only; ownership is established by the
            // handle above, not by this value.
            let _ = writeln!(file, "{}", std::process::id());
            let _ = file.flush();
            Ok(Guard { path, _file: file })
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AlreadyExists
            ) =>
        {
            Err("another instance is active".into())
        }
        Err(error) => Err(format!("could not create instance lock: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("agent-gauge-test-{name}.lock"));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn a_second_acquisition_is_refused_while_the_first_is_held() {
        let path = temp_lock_path("held");

        let first = acquire_at(path.clone()).expect("first acquisition should succeed");
        let second = acquire_at(path.clone());
        assert!(
            second.is_err(),
            "a second instance must not acquire the lock"
        );

        drop(first);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_lock_is_reusable_once_released() {
        let path = temp_lock_path("released");

        let first = acquire_at(path.clone()).expect("first acquisition should succeed");
        drop(first);

        let second = acquire_at(path.clone());
        assert!(
            second.is_ok(),
            "releasing the lock must let the next start acquire it"
        );

        drop(second);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_stale_lock_file_does_not_block_startup() {
        // Simulates a crash: the file survives with no live owner.
        let path = temp_lock_path("stale");
        fs::write(&path, "999999999\n").expect("should be able to write a stale lock");

        let guard = acquire_at(path.clone());
        assert!(
            guard.is_ok(),
            "a lock left behind by a dead process must not lock the user out"
        );

        drop(guard);
        let _ = fs::remove_file(&path);
    }
}
