use super::locks::regular;
use crate::{CoreResult, error::refuse};
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
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
#[derive(Debug, Clone)]
pub struct ProjectKey {
    root: PathBuf,
    channel: StoreChannel,
    store_root: Option<super::StoreRoot>,
    directory: Option<PathBuf>,
    relocation: Option<RelocationObserver>,
}

#[derive(Clone)]
struct RelocationObserver {
    update: Arc<dyn Fn(bool) + Send + Sync>,
}

impl std::fmt::Debug for RelocationObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RelocationObserver")
    }
}

pub(super) struct MovingHistory<'a>(&'a RelocationObserver);

impl Drop for MovingHistory<'_> {
    fn drop(&mut self) {
        (self.0.update)(false);
    }
}
impl PartialEq for ProjectKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for ProjectKey {}
impl PartialOrd for ProjectKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ProjectKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.root, self.channel, &self.store_root).cmp(&(
            &other.root,
            other.channel,
            &other.store_root,
        ))
    }
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
            store_root: None,
            directory: None,
            relocation: None,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Stable path identity for the canonical logical project, shared across
    /// checkout aliases and scoped separately by its consumer.
    pub fn identity(&self) -> String {
        crate::digest(self.root.as_os_str().as_encoded_bytes())[..32].to_owned()
    }
    pub fn channel(&self) -> StoreChannel {
        self.channel
    }
    pub fn directory(&self) -> PathBuf {
        self.directory
            .clone()
            .unwrap_or_else(|| self.anchor_directory())
    }
    pub fn anchor_directory(&self) -> PathBuf {
        self.root.join(self.channel.directory())
    }
    pub fn home_directory(&self) -> Option<PathBuf> {
        self.store_root.as_ref().map(|root| root.directory(self))
    }
    pub fn store_root(&self) -> Option<&super::StoreRoot> {
        self.store_root.as_ref()
    }
    pub fn resolve_existing(self) -> CoreResult<Self> {
        super::location::resolve(
            self,
            super::OpenMode::LiveReadOnly,
            std::time::Duration::from_secs(2),
        )
    }
    /// Observe actual, fenced history relocation on an initializing open.
    /// A successful or failed relocation clears its active status on exit.
    pub fn observe_relocation(mut self, observe: impl Fn(bool) + Send + Sync + 'static) -> Self {
        self.relocation = Some(RelocationObserver { update: Arc::new(observe) });
        self
    }
    pub(super) fn relocation_started(&self) -> Option<MovingHistory<'_>> {
        self.relocation.as_ref().map(|observer| {
            (observer.update)(true);
            MovingHistory(observer)
        })
    }
    pub(crate) fn at_directory(mut self, directory: PathBuf) -> Self {
        self.directory = Some(directory);
        self
    }
    pub(crate) fn with_store_root(mut self, root: super::StoreRoot) -> Self {
        self.store_root = Some(root);
        self
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
