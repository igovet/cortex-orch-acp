//! Metadata-only OpenCode install rejection. Successful verification stays on Python.
//! Do not probe a binary or prepare a jail from this admission predicate.
use super::*;
use std::{os::unix::fs::MetadataExt, path::Path};

pub(super) fn rejected(env: &RouteEnvironment, r: &Value) -> bool {
    if r["engine"] != "opencode" {
        return false;
    }
    let Some(argv) = r["launch_command"].as_array() else {
        return false;
    };
    if argv.is_empty()
        || argv
            .iter()
            .any(|v| v.as_str().is_none_or(|s| s.contains('\0')))
        || !argv[0].as_str().unwrap().starts_with('/')
    {
        return false;
    }
    // install_resolution._resolve_opencode_install never probes these shapes.
    if argv.len() != 2 || argv[1] != "acp" {
        return true;
    }
    let path = Path::new(argv[0].as_str().unwrap());
    let real = match path.canonicalize() {
        Ok(real) => real,
        // Only a stable absent path is qualified. Permission/I/O failures and
        // symlink loops stay with Python rather than inferring their outcome.
        Err(e) => return e.kind() == std::io::ErrorKind::NotFound,
    };
    if real != path {
        return true;
    }
    let Ok(home) = env.home.canonicalize() else {
        return false;
    };
    if [".local/share/app.codeg", ".config/app.codeg"]
        .iter()
        .any(|root| real.starts_with(home.join(root)))
    {
        return true;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.mode() & 0o111 == 0 {
        return true;
    }
    // Match Python's access check as well as mode bits (an executable bit
    // alone does not establish this user's effective execute permission).
    let Ok(cpath) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    if unsafe { libc::access(cpath.as_ptr(), libc::X_OK) } != 0 {
        return true;
    }
    ![unsafe { libc::getuid() }, 0].contains(&meta.uid()) || meta.mode() & 0o022 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };

    #[test]
    fn rejects_only_preprobe_shapes_and_metadata() {
        let root = std::env::temp_dir().join(format!("cw-opencode-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let env = RouteEnvironment {
            home: root.clone(),
            config: root.join("config"),
            state: root.join("state"),
            routing_override: None,
        };
        let binary = root.join("peer");
        let record = |argv: Value| json!({"engine":"opencode","launch_command":argv});
        assert!(rejected(&env, &record(json!([binary]))));
        assert!(rejected(&env, &record(json!([binary, "wrong"]))));
        assert!(rejected(&env, &record(json!([binary, "acp"]))));
        fs::write(&binary, "#!/bin/sh\nexit 91\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!rejected(&env, &record(json!([binary, "acp"]))));
        let alias = root.join("alias");
        symlink(&binary, &alias).unwrap();
        assert!(rejected(&env, &record(json!([alias, "acp"]))));
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o722)).unwrap();
        assert!(rejected(&env, &record(json!([binary, "acp"]))));
        assert!(!rejected(&env, &record(json!(["relative", "acp"]))));
        assert!(!rejected(&env, &record(json!([binary, "\u{0}"]))));
        fs::remove_dir_all(&root).unwrap();
    }
}
