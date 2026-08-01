use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
};

use crate::paths;

pub struct Guard {
    path: PathBuf,
    _file: File,
}

impl Guard {
    pub fn acquire() -> Result<Self, String> {
        let path = paths::state_dir().join("agent-gauge.lock");
        paths::ensure_parent(&path)?;
        match create(&path) {
            Ok(file) => Ok(Self { path, _file: file }),
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
                    .map(|file| Self { path, _file: file })
                    .map_err(|retry| format!("could not create instance lock: {retry}"))
            }
            Err(error) => Err(format!("could not create instance lock: {error}")),
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn create(path: &PathBuf) -> std::io::Result<File> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    writeln!(file, "{}", std::process::id())?;
    file.sync_all()?;
    Ok(file)
}

fn process_exists(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}
