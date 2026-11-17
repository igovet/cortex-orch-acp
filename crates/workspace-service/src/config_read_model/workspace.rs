//! Workspace-only defaults; explicit host root, no machine dispatch or state writer.
use super::{ConfigurationReader, SettingsRead};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path},
};

fn unreadable<T>() -> CoreResult<T> {
    Err(CoreError::Refused("workspace_defaults_unreadable"))
}

impl ConfigurationReader {
    /// Read only mode/checkout from the host-selected Workspace channel state.
    /// This root is independent of XDG engine state, never a socket/model input.
    pub fn workspace_defaults(&self, state: &Path) -> CoreResult<Value> {
        if !state.is_absolute()
            || state
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return unreadable();
        }
        // Missing first-run state is not initialized. Existing ancestors must be
        // real directories; require the channel root itself to be owner-private.
        for parent in state.ancestors() {
            match fs::symlink_metadata(parent) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {
                    if parent == state
                        && (m.uid() != unsafe { libc::getuid() } || m.mode() & 0o077 != 0)
                    {
                        return unreadable();
                    }
                }
                _ => return unreadable(),
            }
        }
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(state.join("defaults.json"))
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
            Err(_) => return unreadable(),
        };
        let m = file.metadata()?;
        const MAX: u64 = 4 * 1024 * 1024;
        if !m.is_file()
            || m.uid() != unsafe { libc::getuid() }
            || m.mode() & 0o077 != 0
            || m.nlink() != 1
            || m.len() > MAX
        {
            return unreadable();
        }
        let mut bytes = Vec::new();
        file.take(MAX + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX {
            return unreadable();
        }
        let doc: Value = serde_json::from_slice(&bytes)
            .map_err(|_| CoreError::Refused("invalid_workspace_defaults"))?;
        let doc = doc
            .as_object()
            .ok_or(CoreError::Refused("invalid_workspace_defaults"))?;
        // Match retained display semantics: absent differs from explicit null.
        // Do not validate or interpret these display values as runtime authority.
        Ok(Value::Object(
            doc.iter()
                .filter(|(k, _)| ["mode", "checkout"].contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ))
    }

    /// Replace Workspace, cached catalog and all five management input seams.
    /// This lower-level seam still accepts setup/project-ceiling; dispatch is unchanged.
    /// settings_with_setup_readers additionally replaces setup discovery.
    pub fn settings_with_workspace_readers(
        &self,
        observations: &Value,
        now: f64,
        project: Option<(&Path, &Path)>,
        workspace_state: &Path,
    ) -> CoreResult<SettingsRead> {
        if !observations.is_object() {
            return Err(CoreError::Refused("invalid_settings_observation"));
        }
        let mut o = observations.clone();
        o["workspace"] = self.workspace_defaults(workspace_state)?;
        self.settings_with_catalog_readers(&o, now, project)
    }
}
