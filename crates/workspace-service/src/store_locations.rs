//! Store placement is selected once by the host, outside request handling.
use cortex_core::{store::StoreRoot, CoreError, CoreResult, ProjectKey, StoreChannel};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

static HOST_ROOT: OnceLock<StoreRoot> = OnceLock::new();

pub(crate) fn channel() -> StoreChannel {
    if workspace_protocol::channel::current().is_dev() {
        StoreChannel::Dev
    } else {
        StoreChannel::Release
    }
}

/// Called by the Workspace and ACP launchers with their trusted channel state.
pub fn configure(state: &Path) -> CoreResult<()> {
    let root = StoreRoot::new(state, channel())?;
    HOST_ROOT
        .set(root.clone())
        .map_err(|_| CoreError::Refused("store_root_already_configured"))?;
    crate::native_store_policy::register_root(&root);
    Ok(())
}

pub(crate) fn key(project: &Path) -> CoreResult<ProjectKey> {
    match HOST_ROOT.get() {
        Some(root) => root.project_key(project),
        // Library fixtures and embedders that haven't configured relocation use
        // explicit checkout-local keys. Both shipped runtimes configure at startup.
        None => ProjectKey::new(project, channel()),
    }
}

pub(crate) fn directory(key: ProjectKey) -> CoreResult<PathBuf> {
    match key.clone().resolve_existing() {
        Ok(current) => Ok(current.directory()),
        Err(error) if crate::local::store_missing(&key, &error) => Ok(key.anchor_directory()),
        Err(error) => Err(error),
    }
}
