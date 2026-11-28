//! Channel identities for the disposable installed-package gate, never host paths.

use super::util::{require, run_ok, sha_bytes};
use crate::json::{parse, Res, Value};
use std::os::unix::fs::MetadataExt;
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
        self.route_state(home).join("cortex/catalog")
    }

    fn route_state(&self, home: &Path) -> PathBuf {
        if self.name == "dev" {
            self.state(home).join("state")
        } else {
            home.join(".local/state")
        }
    }

    pub fn store_anchor(&self, project: &Path) -> PathBuf {
        project.join(&self.store).join("cortex.sqlite3")
    }

    pub fn store_destination(&self, home: &Path, project: &Path) -> PathBuf {
        let basename = project
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let basename: String = basename
            .chars()
            .take(40)
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let basename = if basename.is_empty() {
            "project"
        } else {
            &basename
        };
        let digest = sha_bytes(project.as_os_str().as_encoded_bytes());
        self.route_state(home)
            .join("cortex/projects")
            .join(format!("{basename}-{}", &digest[..32]))
            .join(&self.store)
    }

    pub fn store_db(&self, home: &Path, project: &Path) -> Res<PathBuf> {
        let project = project
            .canonicalize()
            .map_err(|_| "fixture project unavailable")?;
        require(
            project.starts_with(home) && project != home,
            "store project is outside the disposable home",
        )?;
        let metadata = std::fs::metadata(&project).map_err(|_| "fixture project unavailable")?;
        let anchor = self.store_anchor(&project);
        for sidecar in [
            "cortex.sqlite3-wal",
            "cortex.sqlite3-shm",
            "cortex.sqlite3-journal",
        ] {
            require(
                matches!(anchor.with_file_name(sidecar).symlink_metadata(), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "the project anchor has live SQLite sidecars",
            )?;
        }
        let binding = read_binding(&anchor, "12001", metadata.uid())?;
        let directory = self.bound_directory(home, &project, &binding, &metadata)?;
        let db = directory.join("cortex.sqlite3");
        let actual = read_binding(&db, "12", metadata.uid())?;
        require(
            actual == binding,
            "store binding differs from the project anchor",
        )?;
        Ok(db)
    }

    fn bound_directory(
        &self,
        home: &Path,
        project: &Path,
        binding: &Value,
        metadata: &std::fs::Metadata,
    ) -> Res<PathBuf> {
        let directory = self.store_destination(home, project);
        let field = |key| binding.get(key).and_then(Value::as_str);
        let identity = binding.get("root_identity").and_then(Value::as_arr);
        require(
            field("project_root").map(Path::new) == Some(project)
                && field("directory").map(Path::new) == Some(directory.as_path())
                && field("channel") == Some(self.store.as_str())
                && field("store_id").is_some_and(|id| !id.is_empty())
                && identity.is_some_and(|pair| {
                    pair.len() == 2
                        && pair[0].as_i64() == i64::try_from(metadata.dev()).ok()
                        && pair[1].as_i64() == i64::try_from(metadata.ino()).ok()
                }),
            "relocated store path or identity does not match the fixture",
        )?;
        Ok(directory)
    }
}

fn read_binding(db: &Path, expected_version: &str, uid: u32) -> Res<Value> {
    let metadata = db
        .symlink_metadata()
        .map_err(|_| "fixture store file missing")?;
    require(
        metadata.is_file()
            && metadata.uid() == uid
            && metadata.nlink() == 1
            && metadata.mode() & 0o077 == 0
            && db.canonicalize().ok().as_deref() == Some(db),
        "fixture store file is unsafe",
    )?;
    let result = super::native::query(db,
        "PRAGMA application_id; PRAGMA user_version; SELECT binding_json FROM store_location WHERE singleton=1;")?;
    parse_binding(&result, expected_version)
}

fn parse_binding(result: &str, expected_version: &str) -> Res<Value> {
    let mut lines = result.lines();
    require(
        lines.next() == Some("1129467218") && lines.next() == Some(expected_version),
        "wrong project-store format",
    )?;
    let binding = lines.next().ok_or("project-store binding missing")?;
    require(lines.next().is_none(), "ambiguous project-store binding")?;
    parse(binding)
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

#[cfg(test)]
mod tests {
    use super::super::util::{arr, n, obj, s};
    use super::*;

    #[test]
    fn rejects_a_store_in_either_wrong_role_and_ambiguous_binding() {
        let anchor = "1129467218\n12001\n{\"store_id\":\"bound\"}";
        let live = "1129467218\n12\n{\"store_id\":\"bound\"}";
        assert!(parse_binding(anchor, "12001").is_ok());
        assert!(parse_binding(live, "12").is_ok());
        assert!(parse_binding(anchor, "12").is_err());
        assert!(parse_binding(live, "12001").is_err());
        assert!(parse_binding("1129467218\n12001", "12001").is_err());
        assert!(parse_binding(&format!("{live}\n{{}}"), "12").is_err());
    }

    #[test]
    fn relocated_store_binding_must_match_channel_project_and_destination() {
        let home = crate::proc::TempDir::new("installed-store-binding-").unwrap();
        let home = home.0.canonicalize().unwrap();
        let project = home.join("fixture");
        std::fs::create_dir(&project).unwrap();
        let metadata = project.metadata().unwrap();
        let dev = Channel::new("dev").unwrap();
        let digest = sha_bytes(project.as_os_str().as_encoded_bytes());
        let directory = home
            .join(".local/state/cortex-workspace-dev/state/cortex/projects")
            .join(format!("fixture-{}", &digest[..32]))
            .join(".cortex-dev");
        let binding = obj(vec![
            ("store_id", s("42d737a3-1aa7-471a-8fd9-2b8fa0df6469")),
            ("project_root", s(project.to_string_lossy())),
            ("channel", s(".cortex-dev")),
            ("directory", s(directory.to_string_lossy())),
            (
                "root_identity",
                arr(vec![n(metadata.dev() as i64), n(metadata.ino() as i64)]),
            ),
        ]);
        assert_eq!(
            dev.bound_directory(&home, &project, &binding, &metadata)
                .unwrap(),
            directory
        );
        let release = Channel::new("release").unwrap();
        assert!(release
            .bound_directory(&home, &project, &binding, &metadata)
            .is_err());
        for (field, value) in [
            ("project_root", s(home.join("other").to_string_lossy())),
            ("store_id", s("")),
            ("directory", s(home.join("other").to_string_lossy())),
            ("root_identity", arr(vec![n(-1), n(-1)])),
        ] {
            let mut forged = binding.clone();
            forged.set(field, value);
            assert!(
                dev.bound_directory(&home, &project, &forged, &metadata)
                    .is_err(),
                "{field}"
            );
        }
    }
}
