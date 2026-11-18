//! Channel identities for the disposable installed-package gate, never host paths.

use super::util::run_ok;
use crate::json::Res;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Channel {
    pub name: String,
    pub package: String,
    pub unit: String,
    pub service: String,
    pub store: String,
    pub runtime: PathBuf,
    pub socket: PathBuf,
}

impl Channel {
    pub fn new(name: &str) -> Res<Channel> {
        let dev = match name {
            "dev" => true,
            "release" => false,
            _ => return Err("unsupported installed-package channel".into()),
        };
        let package = if dev {
            "cortex-workspace-dev"
        } else {
            "cortex-workspace"
        };
        Ok(Channel {
            name: name.into(),
            package: package.into(),
            unit: format!("{package}.service"),
            service: format!("cortex-workspace-service{}", if dev { "-dev" } else { "" }),
            store: (if dev { ".cortex-dev" } else { ".cortex" }).into(),
            runtime: Path::new("/usr/lib").join(package).join("runtime"),
            socket: Path::new("/run/user/1001")
                .join(package)
                .join("service.sock"),
        })
    }

    pub fn state(&self, home: &Path) -> PathBuf {
        home.join(".local/state").join(&self.package)
    }

    pub fn config(&self, home: &Path) -> PathBuf {
        let base = home.join(".config");
        (if self.name == "dev" {
            base.join(&self.package)
        } else {
            base
        })
        .join("cortex")
    }

    pub fn catalog(&self, home: &Path) -> PathBuf {
        let base = if self.name == "dev" {
            self.state(home).join("state")
        } else {
            home.join(".local/state")
        };
        base.join("cortex/catalog")
    }

    pub fn store_db(&self, project: &Path) -> PathBuf {
        project.join(&self.store).join("cortex.sqlite3")
    }
}

/// Reject mixed channels before launching any container or installing anything.
pub fn require_archives(channel: &Channel, archives: &[&Path]) -> Res<()> {
    for path in archives {
        let out = run_ok(&["dpkg-deb", "-f", &path.to_string_lossy(), "Package"])?;
        if out.stdout.trim() != channel.package {
            return Err("archive package does not match --channel".into());
        }
    }
    Ok(())
}
