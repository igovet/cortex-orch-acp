//! The Workspace channels are disjoint and the checked-in desktop shell is the
//! Dev channel's identity.
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

fn read(relative: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn default_tauri_identity_matches_the_dev_channel() {
    let channel = &read("crates/workspace-protocol/channels.json")["dev"];
    let config = read("apps/cortex-workspace-desktop/src-tauri/tauri.conf.json");
    assert_eq!(config["productName"], channel["title"]);
    assert_eq!(config["identifier"], channel["identifier"]);
}

#[test]
fn the_dev_and_release_channels_share_no_installed_identity() {
    let channels = read("crates/workspace-protocol/channels.json");
    for field in [
        "package",
        "identifier",
        "title",
        "desktop_binary",
        "service_binary",
        "unit",
        "runtime_namespace",
        "config_namespace",
        "state_namespace",
    ] {
        let values: BTreeSet<_> = ["dev", "release"]
            .iter()
            .map(|c| channels[c][field].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(values.len(), 2, "channels share `{field}`");
    }
}
