use super::locks::regular;
use crate::{error::refuse, CoreResult};
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StoreChannel {
    Release,
    Dev,
}
impl StoreChannel {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Release => ".cortex",
            Self::Dev => ".cortex-dev",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectKey {
    root: PathBuf,
    channel: StoreChannel,
}
fn short(path: &Path) -> CoreResult<String> {
    // Git metadata need be owned/single-linked but need not be private mode 0600.
    let m = path.symlink_metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::getuid() } || m.nlink() != 1 || m.len() > 4096 {
        return refuse("project_context_conflict");
    }
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut bytes = Vec::new();
    f.by_ref().take(4097).read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| crate::CoreError::Refused("project_context_conflict"))?;
    let lines: Vec<_> = text.lines().collect();
    if lines.len() != 1 {
        return refuse("project_context_conflict");
    }
    Ok(lines[0].into())
}
fn resolve(base: &Path, raw: &str) -> CoreResult<PathBuf> {
    let p = base.join(raw).canonicalize()?;
    if p.symlink_metadata()?.uid() != unsafe { libc::getuid() } {
        return refuse("unsafe_storage");
    }
    Ok(p)
}
impl ProjectKey {
    pub fn new(root: impl AsRef<Path>, channel: StoreChannel) -> CoreResult<Self> {
        let root = root.as_ref();
        if !root.is_absolute() || !root.is_dir() || root.canonicalize()? != root {
            return refuse("invalid_project");
        }
        let marker = root.join(".git");
        let logical = match marker.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => root.to_path_buf(),
            Err(e) => return Err(e.into()),
            Ok(m) if m.is_dir() && m.uid() == unsafe { libc::getuid() } => root.to_path_buf(),
            Ok(m) if m.is_file() => {
                let text = short(&marker)?;
                let gitdir = resolve(
                    root,
                    text.strip_prefix("gitdir: ")
                        .ok_or(crate::CoreError::Refused("project_context_conflict"))?,
                )?;
                let common = resolve(&gitdir, &short(&gitdir.join("commondir"))?)?;
                let parent = common
                    .parent()
                    .ok_or(crate::CoreError::Refused("project_context_conflict"))?;
                if !common.join("HEAD").is_file()
                    || !common.join("worktrees").is_dir()
                    || gitdir.parent() != Some(common.join("worktrees").as_path())
                    || resolve(&gitdir, &short(&gitdir.join("gitdir"))?)? != marker
                    || parent.join(".git") != common
                {
                    return refuse("project_context_conflict");
                }
                parent.to_path_buf()
            }
            _ => return refuse("unsafe_storage"),
        };
        Ok(Self {
            root: logical,
            channel,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn channel(&self) -> StoreChannel {
        self.channel
    }
    pub fn directory(&self) -> PathBuf {
        self.root.join(self.channel.directory())
    }
    pub fn database(&self) -> PathBuf {
        self.directory().join("cortex.sqlite3")
    }
    /// Recognize historical storage by its layout only. Never open the old
    /// database, descend into task trees, or follow symlinked directories.
    pub(super) fn has_legacy_storage(&self) -> CoreResult<bool> {
        if self.channel != StoreChannel::Release {
            return Ok(false);
        }
        let mut path = self.root.clone();
        for component in [".codex", "cortex"] {
            path.push(component);
            match path.symlink_metadata() {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Ok(false),
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
                Err(e) => return Err(e.into()),
            }
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if matches!(
                name,
                "cortex.sqlite3"
                    | "cortex.sqlite3-wal"
                    | "cortex.sqlite3-shm"
                    | "cortex.sqlite3-journal"
            ) && entry.file_type()?.is_file()
            {
                return Ok(true);
            }
            if let Some(id) = name.strip_prefix("t_") {
                if id.len() == 12
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && entry.file_type()?.is_dir()
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub fn validate_sidecars(&self) -> CoreResult<()> {
        regular(&self.database())?;
        for suffix in ["-wal", "-shm"] {
            let p = self.directory().join(format!("cortex.sqlite3{suffix}"));
            match p.symlink_metadata() {
                Ok(_) => regular(&p)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
