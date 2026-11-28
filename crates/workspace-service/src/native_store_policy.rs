//! Home project-store paths shared by native tools and command containment.
use crate::native_routes::RouteEnvironment;
use cortex_core::{store::StoreRoot, CoreResult, ProjectKey, StoreChannel};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{LazyLock, RwLock},
};

static ACTIVE_ROOTS: LazyLock<RwLock<BTreeSet<PathBuf>>> =
    LazyLock::new(|| RwLock::new(BTreeSet::new()));
static VERIFIED_ANCHORS: LazyLock<RwLock<BTreeMap<PathBuf, (u64, u64, i64, i64)>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

/// Preserve explicit launch roots in the service process, whose own environment
/// can differ from the authority passed to child processes.
pub(crate) fn register_root(root: &StoreRoot) {
    ACTIVE_ROOTS.write().unwrap().insert(root.path().to_owned());
}

pub(crate) fn register_key(key: &ProjectKey) {
    if let Some(root) = key.store_root() {
        register_root(root);
    }
    if key
        .home_directory()
        .is_some_and(|home| home == key.directory())
    {
        let path = key.anchor_directory().join("cortex.sqlite3");
        if let Some(identity) = anchor_identity(&path) {
            VERIFIED_ANCHORS.write().unwrap().insert(path, identity);
        }
    }
}

pub(crate) fn verified_anchor(path: &Path) -> bool {
    let identity = VERIFIED_ANCHORS.read().unwrap().get(path).copied();
    identity.is_some_and(|identity| anchor_identity(path) == Some(identity))
}

pub(crate) fn known_anchor_directories() -> Vec<PathBuf> {
    VERIFIED_ANCHORS
        .read()
        .unwrap()
        .keys()
        .filter_map(|path| path.parent().map(Path::to_owned))
        .collect()
}

fn anchor_identity(path: &Path) -> Option<(u64, u64, i64, i64)> {
    let metadata = path.symlink_metadata().ok()?;
    (metadata.is_file() && metadata.nlink() == 1).then(|| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    })
}

pub(crate) fn roots(env: &BTreeMap<OsString, OsString>) -> CoreResult<BTreeSet<PathBuf>> {
    let mut roots = ACTIVE_ROOTS.read().unwrap().clone();
    if let Some(state) =
        RouteEnvironment::user_state(|name| env.get(std::ffi::OsStr::new(name)).cloned())
    {
        let dev_state = RouteEnvironment::workspace_dev_state(
            &workspace_protocol::channel::dev().state_dir(&state),
            None,
        );
        for (state, channel) in [
            (&state, StoreChannel::Release),
            (&dev_state, StoreChannel::Dev),
        ] {
            roots.insert(StoreRoot::new(state, channel)?.path().to_owned());
        }
    }
    Ok(roots)
}

pub(crate) fn contains(root: &Path, path: &Path) -> bool {
    path.starts_with(root) || root.canonicalize().is_ok_and(|real| path.starts_with(real))
}

pub(crate) fn in_home_store(env: &BTreeMap<OsString, OsString>, path: &Path) -> bool {
    roots(env).map_or(true, |roots| roots.iter().any(|root| contains(root, path)))
}
