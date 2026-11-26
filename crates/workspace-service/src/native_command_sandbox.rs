//! Worker command profiles. Every profile runs the command on the host at its real
//! paths under a Landlock ruleset; see `native_command_live` and `native_sandbox`.
//! There is no namespace or mount view and no unconfined fallback: a ruleset that
//! cannot be prepared refuses the launch.
pub(crate) use crate::native_command_live::policy;
pub(crate) use crate::native_command_live::Sandbox;
#[path = "native_command_devices.rs"]
pub(crate) mod devices;
pub(crate) const CODEX_SCRATCH: &str = "codex_scratch";
pub(crate) const WORKTREE_READ_ONLY: &str = "worktree_read_only";
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    ReadOnly,
    Guarded,
    Development,
    /// Development plus a writable `.cortex`/`.cortex-dev` whose live store files
    /// stay hidden.
    Auto,
}

impl Profile {
    /// Development and Auto also write the shared `/tmp` and `/var/tmp`.
    pub(crate) fn wide(self) -> bool {
        matches!(self, Self::Development | Self::Auto)
    }
}

fn unavailable(reason: &str) -> io::Error {
    io::Error::other(format!("native_command_sandbox_unavailable: {reason}"))
}

impl Sandbox {
    /// A managed Codex command: the read-only profile at the real checkout.
    pub(crate) fn prepare_scoped(root: &Path, protected: &[PathBuf]) -> io::Result<Self> {
        let mut s = Self::prepare(root, Profile::ReadOnly, &[], protected, false)?;
        s.confine_cwd();
        Ok(s)
    }

    /// A command in a linked task worktree: the read-only profile at the worktree.
    /// The main checkout stays readable; its live stores stay hidden.
    pub(crate) fn prepare_worktree(
        root: &Path,
        logical: &Path,
        protected: &[PathBuf],
    ) -> io::Result<Self> {
        if root.canonicalize()? != root
            || logical.canonicalize()? != logical
            || root.starts_with(logical)
            || logical.starts_with(root)
            || !cortex_core::ProjectKey::new(root, cortex_core::StoreChannel::Release)
                .is_ok_and(|key| key.root() == logical)
        {
            return Err(unavailable("invalid_worktree_view"));
        }
        let mut s = Self::prepare(root, Profile::ReadOnly, &[], protected, false)?;
        s.confine_cwd();
        Ok(s)
    }

    /// A language server: the read-only profile with private build output and an
    /// offline Cargo.
    pub(crate) fn prepare_language(root: &Path, rust: bool) -> io::Result<Self> {
        if root.canonicalize()? != root {
            return Err(unavailable("invalid_language_view"));
        }
        let mut environment: BTreeMap<OsString, OsString> = crate::tool_path::environment();
        if rust {
            environment.insert("CARGO_NET_OFFLINE".into(), "true".into());
        }
        let mut s = Self::with_environment(root, Profile::ReadOnly, &[], &[], false, environment)?;
        let target = s.scratch().join("target");
        s.set_environment("CARGO_TARGET_DIR", target.into_os_string());
        Ok(s)
    }
}
