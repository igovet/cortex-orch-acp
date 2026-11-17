//! Bounded package verification shared by the catalog-only Codex/Claude producers.
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn symlinked_home_cannot_hide_an_untrusted_install_root() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let scratch = Scratch::new().unwrap();
        let real_home = scratch.0.join("real-home");
        let package = real_home.join(".config/app.codeg/adapter");
        fs::create_dir_all(&package).unwrap();
        let home = scratch.0.join("home-alias");
        symlink(&real_home, &home).unwrap();
        let node = scratch.0.join("node");
        fs::write(&node, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        let entry = package.join("index.js");
        fs::write(&entry, "// synthetic").unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"version":"0.31.1","bin":{"claude-agent-acp":"index.js"}}"#,
        )
        .unwrap();
        let env = RouteEnvironment {
            home,
            config: scratch.0.join("config"),
            state: scratch.0.join("state"),
            routing_override: None,
        };
        let r = json!({"engine":"claude-agent-acp","launch_command":[node,entry]});
        assert!(claude(&env, &r).is_none());
    }
}
use super::*;
use sha2::Digest;

pub(super) struct ClaudeInstall(pub(super) Vec<(PathBuf, String)>);
impl ClaudeInstall {
    pub(super) fn unchanged(&self) -> bool {
        self.0.iter().all(|(path, hash)| {
            path.canonicalize().is_ok_and(|real| real == *path)
                && digest(path).as_ref() == Some(hash)
        })
    }
}
pub(super) fn plain(path: &Path) -> Option<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 4 * 1024 * 1024).then_some(bytes)
}
pub(super) fn digest(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let before = file.metadata().ok()?;
    if !before.is_file() || before.len() > 512 * 1024 * 1024 {
        return None;
    }
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut bytes).ok()?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > 512 * 1024 * 1024 {
            return None;
        }
        hash.update(&bytes[..n]);
    }
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
            m.mode(),
        )
    };
    if size != before.len() || identity(&before) != identity(&file.metadata().ok()?) {
        return None;
    }
    Some(format!("{:x}", hash.finalize()))
}
pub(super) fn candidate(a: &[Value]) -> Option<PathBuf> {
    if a.len() != 2 {
        return None;
    }
    Path::new(a[1].as_str()?)
        .parent()?
        .ancestors()
        .take(6)
        .map(|p| p.join("package.json"))
        .find(|p| p.is_file())
}
pub(super) fn claude(env: &RouteEnvironment, r: &Value) -> Option<ClaudeInstall> {
    if r["engine"] != "claude-agent-acp" {
        return None;
    }
    let a = r["launch_command"].as_array()?;
    let manifest = candidate(a)?;
    let node = PathBuf::from(a[0].as_str()?);
    let entry = PathBuf::from(a[1].as_str()?);
    let paths = [node, entry, manifest];
    let home = env.home.canonicalize().ok()?;
    for path in &paths {
        if !path.is_absolute()
            || path.canonicalize().ok()?.as_path() != path
            || [".local/share/app.codeg", ".config/app.codeg"]
                .iter()
                .any(|root| path.starts_with(home.join(root)))
        {
            return None;
        }
    }
    use std::os::unix::fs::PermissionsExt;
    if fs::metadata(&paths[0]).ok()?.permissions().mode() & 0o111 == 0 {
        return None;
    }
    let node = digest(&paths[0])?;
    let entry = digest(&paths[1])?;
    let manifest = plain(&paths[2])?;
    let data: Value = serde_json::from_slice(&manifest).ok()?;
    if data["version"].as_str()?.is_empty() {
        return None;
    }
    let bin = data["bin"].as_object()?;
    // Other bin representations remain with the Python verifier.
    if bin.is_empty()
        || !bin.values().any(|v| {
            v.as_str().is_some_and(|s| {
                paths[2]
                    .parent()
                    .unwrap()
                    .join(s)
                    .canonicalize()
                    .is_ok_and(|p| p == paths[1])
            })
        })
    {
        return None;
    }
    Some(ClaudeInstall(
        paths
            .into_iter()
            .zip([
                node,
                entry,
                format!("{:x}", sha2::Sha256::digest(&manifest)),
            ])
            .collect(),
    ))
}
