//! Subprocess and filesystem helpers for the build path.

use crate::json::Res;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run a command to completion with a deadline; a non-zero exit or timeout is an error.
pub fn run(command: &mut Command, timeout: Duration) -> Res<()> {
    let label = format!("{:?}", command.get_program());
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {label}: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => return Err(format!("{label} failed: {status}")),
            None if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label} timed out"));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

pub fn output(command: &mut Command) -> Res<String> {
    let label = format!("{:?}", command.get_program());
    let out = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot start {label}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{label} failed: {}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|_| format!("{label} output is not UTF-8"))
}

pub fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.permissions().mode())
        .unwrap_or(0)
}

pub fn chmod(path: &Path, mode: u32) -> Res<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

/// `shutil.copytree`; with `preserve_symlinks` links are copied as links, otherwise followed.
pub fn copy_tree(src: &Path, dst: &Path, preserve_symlinks: bool) -> Res<()> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src)
        .map_err(|e| format!("cannot read {}: {e}", src.display()))?
        .flatten()
    {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() && preserve_symlinks {
            let target = fs::read_link(&from).map_err(|e| e.to_string())?;
            std::os::unix::fs::symlink(target, &to).map_err(|e| e.to_string())?;
        } else if from.is_dir() {
            copy_tree(&from, &to, preserve_symlinks)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// A uniquely named scratch directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> Res<TempDir> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("{prefix}{}-{nanos}", std::process::id()));
        fs::create_dir(&path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        Ok(TempDir(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
