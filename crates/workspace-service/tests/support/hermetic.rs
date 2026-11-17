//! Linked into every test process: before `main` (so before any thread exists) the process
//! gets a private HOME and XDG tree, so no test can read or write the owner's own state
//! (`~/.local/state/cortex-workspace*`, `~/.config`, the runtime directory).
#![allow(dead_code)]

extern "C" fn private_home() {
    // Only the cargo-run test process itself. A peer the test starts with a cleared
    // environment is not cargo-run; one that inherits keeps its parent's private home.
    if std::env::var_os("CARGO_MANIFEST_DIR").is_none()
        || std::env::var_os("HOME")
            .is_some_and(|h| h.as_encoded_bytes().starts_with(b"/tmp/cw-test-home-"))
    {
        return;
    }
    unsafe {
        let mut template = *b"/tmp/cw-test-home-XXXXXX\0";
        let dir = libc::mkdtemp(template.as_mut_ptr().cast());
        if dir.is_null() {
            libc::abort();
        }
        let base = std::ffi::CStr::from_ptr(dir).to_str().unwrap().to_owned();
        for (key, leaf) in [
            ("HOME", ""),
            ("XDG_CONFIG_HOME", "/config"),
            ("XDG_STATE_HOME", "/state"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_CACHE_HOME", "/cache"),
            ("XDG_RUNTIME_DIR", "/run"),
        ] {
            let path = format!("{base}{leaf}");
            if !leaf.is_empty() {
                std::fs::create_dir(&path).unwrap();
                std::fs::set_permissions(
                    &path,
                    std::os::unix::fs::PermissionsExt::from_mode(0o700),
                )
                .unwrap();
            }
            std::env::set_var(key, path);
        }
        REMOVE.store(
            Box::into_raw(Box::new(base)) as usize,
            std::sync::atomic::Ordering::SeqCst,
        );
        libc::atexit(remove_private_home);
    }
}
static REMOVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
extern "C" fn remove_private_home() {
    let pointer = REMOVE.swap(0, std::sync::atomic::Ordering::SeqCst) as *mut String;
    if !pointer.is_null() {
        let base = unsafe { Box::from_raw(pointer) };
        let _ = std::process::Command::new("/bin/rm")
            .args(["-rf", "--"])
            .arg(&*base)
            .status();
    }
}
#[used]
#[link_section = ".init_array"]
static PRIVATE_HOME: extern "C" fn() = private_home;
