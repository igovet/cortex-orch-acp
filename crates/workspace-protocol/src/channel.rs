//! Single compiled identity table shared by service, native shell and packager.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Deserialize)]
pub struct Channel {
    pub name: String,
    pub package: String,
    pub identifier: String,
    pub title: String,
    pub desktop_binary: String,
    pub service_binary: String,
    pub unit: String,
    pub runtime_namespace: String,
    pub config_namespace: String,
    pub state_namespace: String,
}

pub fn current() -> &'static Channel {
    static TABLE: OnceLock<BTreeMap<String, Channel>> = OnceLock::new();
    &TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("../channels.json")).expect("channel table")
    })[env!("CORTEX_WORKSPACE_CHANNEL")]
}
impl Channel {
    pub fn is_dev(&self) -> bool {
        self.name == "dev"
    }
    pub fn runtime_dir(&self, base: &Path) -> PathBuf {
        base.join(&self.runtime_namespace)
    }
    pub fn state_dir(&self, base: &Path) -> PathBuf {
        base.join(&self.state_namespace)
    }
    pub fn config_dir(&self, base: &Path) -> PathBuf {
        base.join(&self.config_namespace)
    }
}

#[test]
fn default_channel_is_visible_and_paths_are_disjoint() {
    let table: BTreeMap<String, Channel> =
        serde_json::from_str(include_str!("../channels.json")).unwrap();
    assert!(table.contains_key(&current().name));
    let d = &table["dev"];
    let r = &table["release"];
    for (left, right) in [
        (
            d.runtime_dir(Path::new("/runtime")).join("service.sock"),
            r.runtime_dir(Path::new("/runtime")).join("service.sock"),
        ),
        (
            d.runtime_dir(Path::new("/runtime")).join("service.lock"),
            r.runtime_dir(Path::new("/runtime")).join("service.lock"),
        ),
        (
            d.state_dir(Path::new("/state")).join("projects.json"),
            r.state_dir(Path::new("/state")).join("projects.json"),
        ),
        (
            d.config_dir(Path::new("/config")).join("cortex"),
            PathBuf::from("/config/cortex"),
        ),
        (
            d.state_dir(Path::new("/state")).join("cortex"),
            PathBuf::from("/state/cortex"),
        ),
    ] {
        assert_ne!(left, right);
    }
}
