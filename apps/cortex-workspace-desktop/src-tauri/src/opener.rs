//! Handing a folder, file or URL to another program. Checkout actions use the service's targets;
//! message links use a separate validated target after explicit UI activation.
use std::{
    ffi::{OsStr, OsString},
    fmt::Write as _,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{process::Command, time::timeout};

/// VS Code's own launcher and the builds that take the same arguments, best first.
const VS_CODE: &[&str] = &["code", "code-insiders", "codium"];
/// A window started from the desktop can have a shorter PATH than a terminal, so the usual homes are also tried.
const USUAL_FOLDERS: &[&str] = &[
    "/usr/bin",
    "/usr/local/bin",
    "/snap/bin",
    "/usr/share/code/bin",
];
/// The freedesktop opener, then GLib's. Absolute, so a project cannot put its own first.
const SYSTEM_OPENERS: &[(&str, &[&str])] =
    &[("/usr/bin/xdg-open", &[]), ("/usr/bin/gio", &["open"])];
/// A launcher that keeps running has started; waiting longer would only report a working window as a failure.
const HANDOVER: Duration = Duration::from_secs(8);

fn runnable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn find_vs_code(path: Option<&OsStr>, usual: &[&str]) -> Option<PathBuf> {
    let mut folders: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    folders.extend(usual.iter().map(PathBuf::from));
    VS_CODE.iter().find_map(|name| {
        folders
            .iter()
            .map(|folder| folder.join(name))
            .find(|candidate| runnable(candidate))
    })
}

fn find_vs_code_for_home(
    path: Option<&OsStr>,
    usual: &[&str],
    home: Option<&Path>,
) -> Option<PathBuf> {
    find_vs_code(path, usual).or_else(|| {
        let home = home?;
        let folders = [home.join(".local/bin"), home.join(".local/share/code/bin")];
        VS_CODE
            .iter()
            .find_map(|name| folders.iter().map(|p| p.join(name)).find(|p| runnable(p)))
    })
}

pub fn vs_code() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    find_vs_code_for_home(
        std::env::var_os("PATH").as_deref(),
        USUAL_FOLDERS,
        home.as_deref(),
    )
}

pub fn find_system_opener(
    openers: &[(&'static str, &'static [&'static str])],
) -> Option<(PathBuf, &'static [&'static str])> {
    openers
        .iter()
        .map(|(path, args)| (PathBuf::from(path), *args))
        .find(|(path, _)| runnable(path))
}

pub fn system_opener() -> Option<(PathBuf, &'static [&'static str])> {
    find_system_opener(SYSTEM_OPENERS)
}

const GDBUS: &str = "/usr/bin/gdbus";
const FILE_MANAGER: &str = "org.freedesktop.FileManager1";

fn bus_call(destination: &str, object: &str, method: &str) -> Command {
    let mut call = Command::new(GDBUS);
    call.args([
        "call",
        "--session",
        "--timeout",
        "3",
        "--dest",
        destination,
        "--object-path",
        object,
        "--method",
        method,
    ])
    .kill_on_drop(true)
    .stdin(Stdio::null())
    .stderr(Stdio::null());
    call
}

/// Probe the bus, not the file manager: capability discovery must not start a file manager.
pub async fn file_manager_available() -> bool {
    if !runnable(Path::new(GDBUS)) {
        return false;
    }
    for method in [
        "org.freedesktop.DBus.ListNames",
        "org.freedesktop.DBus.ListActivatableNames",
    ] {
        let mut call = bus_call("org.freedesktop.DBus", "/org/freedesktop/DBus", method);
        match timeout(Duration::from_secs(4), call.output()).await {
            Ok(Ok(reply)) if reply.status.success() => {
                if String::from_utf8_lossy(&reply.stdout).contains("'org.freedesktop.FileManager1'")
                {
                    return true;
                }
            }
            _ => return false,
        }
    }
    false
}

fn show_items_arg(file: &str) -> String {
    // Preserve path separators and encode the UTF-8 bytes of everything but RFC 3986
    // unreserved characters. This also excludes all GVariant string delimiters.
    let mut uri = String::from("['file://");
    for byte in file.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            write!(uri, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    uri.push_str("']");
    uri
}

fn show_items_call(file: &str) -> Command {
    let mut call = bus_call(
        FILE_MANAGER,
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1.ShowItems",
    );
    call.arg(show_items_arg(file)).arg("").stdout(Stdio::null());
    call
}

/// Use the resolved file only for selection. The default opener sees only the directory.
pub async fn reveal_file(file: &str) -> Result<(), String> {
    let path = Path::new(file);
    let folder = path
        .parent()
        .filter(|folder| folder.is_absolute())
        .ok_or("This file could not be found.")?;
    if runnable(Path::new(GDBUS))
        && matches!(
            timeout(Duration::from_secs(4), show_items_call(file).status()).await,
            Ok(Ok(status)) if status.success()
        )
    {
        return Ok(());
    }
    let (program, prefix) = system_opener().ok_or("No file manager could show this file.")?;
    let mut args: Vec<OsString> = prefix.iter().map(Into::into).collect();
    args.push(folder.as_os_str().into());
    launch(
        &program,
        &args,
        "The file manager could not open this folder.",
    )
    .await
}

/// One plain `https://host/path` address a browser may be handed: no credentials, no
/// whitespace or control characters, and nothing an opener could read as an option.
pub fn web_address(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    url.len() <= 2048
        && !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Message links are untrusted data, even when the owner explicitly chooses to open one.
#[cfg(any(feature = "desktop", test))]
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum ResultTarget {
    Url {
        url: String,
    },
    File {
        path: String,
        line: Option<u64>,
        column: Option<u64>,
    },
}

#[cfg(any(feature = "desktop", test))]
fn result_url(url: &str) -> bool {
    if url.len() > 4096
        || url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("mailto") {
        return !rest.starts_with("//")
            && rest
                .split('?')
                .next()
                .is_some_and(|address| !address.is_empty());
    }
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") {
        return rest.strip_prefix("//").is_some_and(|rest| {
            let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
            !host.is_empty() && !host.contains('@')
        });
    }
    false
}

#[cfg(any(feature = "desktop", test))]
fn result_file(path: &str) -> Result<(PathBuf, std::fs::Metadata), String> {
    let requested = Path::new(path);
    if !requested.is_absolute() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err("Only an absolute local file path can be opened.".into());
    }
    let artifact = result_artifact(requested);
    let private = "Files in private Cortex or Git directories cannot be opened from a result.";
    if artifact && private_artifact_path(requested) {
        return Err(private.into());
    }
    let path = std::fs::canonicalize(requested)
        .map_err(|_| "This file does not exist or could not be opened.")?;
    let metadata = path
        .metadata()
        .map_err(|_| "This file could not be opened.")?;
    if !metadata.is_file() {
        return Err("Only an existing regular file can be opened.".into());
    }
    if artifact && private_artifact_path(&path) {
        return Err(private.into());
    }
    Ok((path, metadata))
}

// Match the renderer's video and subtitle paths. The canonical target must
// also be an artifact to take the default-app path.
#[cfg(any(feature = "desktop", test))]
fn result_artifact(path: &Path) -> bool {
    const EXTENSIONS: &[&str] = &[
        "mp4", "m4v", "webm", "ogv", "mov", "mkv", "avi", "srt", "vtt", "ass", "ssa",
    ];
    path.extension()
        .is_some_and(|ext| EXTENSIONS.iter().any(|kind| ext.eq_ignore_ascii_case(kind)))
}

#[cfg(any(feature = "desktop", test))]
fn private_artifact_path(path: &Path) -> bool {
    // An external worktree home may be named .cortex without being a project store.
    // Only a missing marker clears the check; do not read or follow marker contents.
    let marked = |path: PathBuf| {
        !matches!(
            path.symlink_metadata(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound
        )
    };
    path.ancestors()
        .skip(1)
        .any(|dir| match dir.file_name().and_then(OsStr::to_str) {
            Some(".git") => true,
            Some(".cortex") => marked(dir.join("cortex.sqlite3")),
            Some(".cortex-dev") => {
                marked(dir.join("cortex.sqlite3"))
                    || marked(dir.join("owner.json"))
                    || dir
                        .parent()
                        .is_some_and(|project| marked(project.join(".cortex-dev-owner.json")))
            }
            _ => false,
        })
}

#[cfg(any(feature = "desktop", test))]
pub(crate) async fn open_result_target(target: ResultTarget) -> Result<(), String> {
    open_result_target_with(target, vs_code, system_opener).await
}

#[cfg(any(feature = "desktop", test))]
async fn open_result_target_with(
    target: ResultTarget,
    editor: impl FnOnce() -> Option<PathBuf>,
    system: impl FnOnce() -> Option<(PathBuf, &'static [&'static str])>,
) -> Result<(), String> {
    match target {
        ResultTarget::Url { url } => {
            if !result_url(&url) {
                return Err("Only HTTP, HTTPS and mail links can be opened.".into());
            }
            let (program, prefix) = system().ok_or("No system link opener is installed.")?;
            let mut args: Vec<OsString> = prefix.iter().map(Into::into).collect();
            args.push(url.into());
            launch(&program, &args, "The system could not open this link.").await
        }
        ResultTarget::File { path, line, column } => {
            let artifact = result_artifact(Path::new(&path));
            let (file, meta) = result_file(&path)?;
            if artifact && !result_artifact(&file) {
                return Err("This link does not resolve to a video or subtitle file.".into());
            }
            if let Some(editor) = if artifact { None } else { editor() } {
                let args = if let Some(line) = line.filter(|line| *line > 0) {
                    let mut target = file.into_os_string();
                    target.push(format!(":{line}"));
                    if let Some(column) = column.filter(|column| *column > 0) {
                        target.push(format!(":{column}"));
                    }
                    vec!["--reuse-window".into(), "--goto".into(), target]
                } else {
                    vec!["--reuse-window".into(), "--".into(), file.into_os_string()]
                };
                return launch(&editor, &args, "VS Code could not open this file.").await;
            }
            // A system file handler must not be used to hand off a launcher or an executable.
            if meta.permissions().mode() & 0o111 != 0
                || file
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"))
            {
                return Err(
                    "This file needs an editor; it was not sent to the default app.".into(),
                );
            }
            let (program, prefix) = system().ok_or(if artifact {
                "No system file opener is installed."
            } else {
                "No editor or system file opener is installed."
            })?;
            let mut args: Vec<OsString> = prefix.iter().map(Into::into).collect();
            args.push(file.into_os_string());
            launch(&program, &args, "The default app could not open this file.").await
        }
    }
}

#[cfg(test)]
mod result_target_tests {
    use super::*;

    #[tokio::test]
    async fn rejects_untrusted_targets_before_launch() {
        let bad =
            serde_json::json!({"kind": "url", "url": "file:///tmp/example", "program": "other"});
        assert!(serde_json::from_value::<ResultTarget>(bad).is_err());
        let url = ResultTarget::Url {
            url: "file:///tmp/example".into(),
        };
        assert_eq!(
            open_result_target(url).await.unwrap_err(),
            "Only HTTP, HTTPS and mail links can be opened."
        );
        let missing = std::env::current_exe().unwrap().join("missing");
        let file = ResultTarget::File {
            path: missing.to_str().unwrap().into(),
            line: Some(2),
            column: None,
        };
        assert_eq!(
            open_result_target(file).await.unwrap_err(),
            "This file does not exist or could not be opened."
        );
    }

    #[tokio::test]
    async fn hands_artifacts_to_the_default_app_as_one_argument_and_reports_failure() {
        let root = super::tests::folder("artifact-handoff");
        let output = root.join(".cortex/worktrees/task/out");
        std::fs::create_dir_all(&output).unwrap();
        for name in ["clip.MP4", "sub titles.SRT"] {
            let path = output.join(name);
            std::fs::write(&path, "fixture").unwrap();
            let file = || ResultTarget::File {
                path: path.to_str().unwrap().into(),
                line: None,
                column: None,
            };
            let editor = || panic!("artifact must not consult the editor");
            let app = || {
                const PREFIX: &[&str] = &[
                    "-c",
                    r#"test "$#" = 1 && case "${1##*/}" in clip.MP4|"sub titles.SRT") test -f "$1";; *) exit 4;; esac"#,
                    "fixture-opener",
                ];
                Some(("/bin/sh".into(), PREFIX))
            };
            open_result_target_with(file(), editor, app).await.unwrap();
            let unavailable = || None;
            assert_eq!(
                open_result_target_with(file(), editor, unavailable).await,
                Err("No system file opener is installed.".into())
            );
            let failed = || {
                const PREFIX: &[&str] = &["-c", "exit 3", "fixture-opener"];
                Some(("/bin/sh".into(), PREFIX))
            };
            assert_eq!(
                open_result_target_with(file(), editor, failed).await,
                Err("The default app could not open this file.".into())
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn refuses_unsafe_artifact_targets_before_any_launch() {
        let root = super::tests::folder("result-artifacts");
        let executable = root.join("clip.MP4");
        let subtitle = root.join("sub titles.SRT");
        let alias = root.join("alias.srt");
        super::tests::program(&executable, "", 0o700);
        std::fs::write(&subtitle, "fixture").unwrap();
        let non_artifact = root.join("handler.desktop");
        std::fs::write(&non_artifact, "").unwrap();
        std::os::unix::fs::symlink(&non_artifact, &alias).unwrap();
        let release = root.join(".cortex");
        std::fs::create_dir(&release).unwrap();
        std::fs::write(release.join("cortex.sqlite3"), "").unwrap();
        let private_video = release.join("private.mp4");
        std::fs::write(&private_video, "fixture").unwrap();
        let requested_private_alias = release.join("outside.srt");
        std::os::unix::fs::symlink(&subtitle, &requested_private_alias).unwrap();

        let dev = root.join(".cortex-dev");
        std::fs::create_dir(&dev).unwrap();
        std::fs::write(dev.join("owner.json"), "").unwrap();
        let private_subtitle = dev.join("private.srt");
        std::fs::write(&private_subtitle, "fixture").unwrap();
        let canonical_private_alias = root.join("private-alias.srt");
        std::os::unix::fs::symlink(&private_subtitle, &canonical_private_alias).unwrap();

        let legacy_project = root.join("legacy");
        let legacy_dev = legacy_project.join(".cortex-dev");
        std::fs::create_dir_all(&legacy_dev).unwrap();
        std::fs::write(legacy_project.join(".cortex-dev-owner.json"), "").unwrap();
        let legacy_subtitle = legacy_dev.join("private.srt");
        std::fs::write(&legacy_subtitle, "fixture").unwrap();

        let git = root.join(".git");
        std::fs::create_dir(&git).unwrap();
        let git_video = git.join("private.mp4");
        std::fs::write(&git_video, "fixture").unwrap();
        for path in [&executable, &subtitle] {
            assert!(result_artifact(path));
            assert!(result_artifact(
                &result_file(path.to_str().unwrap()).unwrap().0
            ));
        }
        for (file, expected) in [
            (
                executable,
                "This file needs an editor; it was not sent to the default app.",
            ),
            (
                alias,
                "This link does not resolve to a video or subtitle file.",
            ),
            (
                private_video,
                "Files in private Cortex or Git directories cannot be opened from a result.",
            ),
            (
                private_subtitle,
                "Files in private Cortex or Git directories cannot be opened from a result.",
            ),
            (
                requested_private_alias,
                "Files in private Cortex or Git directories cannot be opened from a result.",
            ),
            (
                canonical_private_alias,
                "Files in private Cortex or Git directories cannot be opened from a result.",
            ),
            (
                legacy_subtitle,
                "Files in private Cortex or Git directories cannot be opened from a result.",
            ),
            (
                git_video,
                "Files in private Cortex or Git directories cannot be opened from a result.",
            ),
        ] {
            assert_eq!(
                open_result_target_with(
                    ResultTarget::File {
                        path: file.into_os_string().into_string().unwrap(),
                        line: None,
                        column: None,
                    },
                    || panic!("refused artifact must not consult the editor"),
                    || panic!("refused artifact must not consult the opener"),
                )
                .await
                .unwrap_err(),
                expected
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_result_urls_and_files_without_launching() {
        for url in [
            "http://example.invalid/a",
            "HTTPS://example.invalid:8443/a?foo=bar",
            "mailto:person@example.invalid",
        ] {
            assert!(result_url(url), "{url}");
        }
        for url in [
            "--option",
            "javascript:alert",
            "file:///etc/passwd",
            "//example.invalid",
            "https://",
            "https://u@example.invalid",
            "https://example.invalid\nargument",
            "mailto://other",
        ] {
            assert!(!result_url(url), "{url}");
        }
        let file = std::env::current_exe().unwrap();
        assert_eq!(
            result_file(file.to_str().unwrap()).unwrap().0,
            file.canonicalize().unwrap()
        );
        assert!(result_file("relative.txt").is_err());
        assert!(result_file("/").is_err());
        assert!(result_file(file.join("missing").to_str().unwrap()).is_err());
    }
}

/// VS Code's arguments for a folder, or for a file at a line inside that folder.
pub fn vs_code_args(root: &str, file: Option<&str>, line: u64) -> Vec<OsString> {
    match file {
        None => ["--reuse-window", "--", root].map(OsString::from).to_vec(),
        Some(file) => vec![
            "--reuse-window".into(),
            root.into(),
            "--goto".into(),
            format!("{file}:{line}").into(),
        ],
    }
}

/// Runs one opener. Its own failure is the answer; a launcher still running after the wait has handed over.
pub async fn launch(program: &Path, args: &[OsString], failed: &'static str) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| failed)?;
    match timeout(HANDOVER, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(_) => Err(failed.into()),
        Err(_) => Ok(()),
    }
}

/// Login uses the desktop session environment and real HOME, never a copied credential directory.
pub async fn engine_login(program: &str, args: &[OsString]) -> Result<(), String> {
    let terminal = Path::new("/usr/bin/gnome-terminal");
    if !runnable(terminal) {
        return Err(
            "Native login needs gnome-terminal on this machine. No login was started.".into(),
        );
    }
    let mut command: Vec<OsString> = vec!["--".into(), program.into()];
    command.extend_from_slice(args);
    launch(
        terminal,
        &command,
        "The native login terminal could not open.",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_https_address_goes_to_the_browser() {
        assert!(web_address("https://github.com/owner/repo/pull/7"));
        for url in [
            "http://github.com/owner/repo/pull/7",
            "file:///etc/passwd",
            "--help",
            "https://user:pw@github.com/x",
            "https://github.com/a b",
            "https://github.com/a\nb",
            "https:///path",
            "javascript:alert(1)",
        ] {
            assert!(!web_address(url), "{url}");
        }
    }

    #[test]
    fn show_items_uses_one_encoded_file_uri_and_an_empty_startup_id() {
        let call = show_items_call("/tmp/é space#%'.txt");
        let cmd = call.as_std();
        assert_eq!(cmd.get_program(), "/usr/bin/gdbus");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [
                "call",
                "--session",
                "--timeout",
                "3",
                "--dest",
                "org.freedesktop.FileManager1",
                "--object-path",
                "/org/freedesktop/FileManager1",
                "--method",
                "org.freedesktop.FileManager1.ShowItems",
                "['file:///tmp/%C3%A9%20space%23%25%27.txt']",
                "",
            ]
        );
    }

    pub(super) fn folder(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cw-opener-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    pub(super) fn program(path: &Path, body: &str, mode: u32) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn vs_code_is_found_on_the_path_and_in_its_usual_homes_only_when_runnable() {
        let root = folder("find");
        let (on_path, usual) = (root.join("path"), root.join("usual"));
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(&usual).unwrap();
        let usual_text = usual.to_str().unwrap().to_owned();
        assert_eq!(
            find_vs_code(Some(on_path.as_os_str()), &[&usual_text]),
            None
        );
        program(&on_path.join("code"), "", 0o600);
        assert_eq!(
            find_vs_code(Some(on_path.as_os_str()), &[&usual_text]),
            None,
            "a plain file named code is not an editor"
        );
        program(&usual.join("codium"), "", 0o700);
        assert_eq!(
            find_vs_code(Some(on_path.as_os_str()), &[&usual_text]),
            Some(usual.join("codium"))
        );
        program(&on_path.join("code"), "", 0o700);
        assert_eq!(
            find_vs_code(Some(on_path.as_os_str()), &[&usual_text]),
            Some(on_path.join("code"))
        );
        assert_eq!(
            find_vs_code(None, &[&usual_text]),
            Some(usual.join("codium"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn desktop_finds_user_launcher_even_with_a_short_path() {
        let root = folder("home");
        let bin = root.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        program(&bin.join("code"), "", 0o700);
        assert_eq!(
            find_vs_code_for_home(None, &[], Some(&root)),
            Some(bin.join("code"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_first_runnable_system_opener_wins_and_none_is_an_answer() {
        let root = folder("system");
        let absent: &'static str = Box::leak(
            root.join("absent")
                .to_str()
                .unwrap()
                .to_owned()
                .into_boxed_str(),
        );
        let present: &'static str = Box::leak(
            root.join("gio")
                .to_str()
                .unwrap()
                .to_owned()
                .into_boxed_str(),
        );
        assert!(find_system_opener(&[(absent, &[])]).is_none());
        program(Path::new(present), "", 0o700);
        assert_eq!(
            find_system_opener(&[(absent, &[]), (present, &["open"])]),
            Some((PathBuf::from(present), &["open"][..]))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_folder_ends_the_options_and_a_file_opens_at_its_line() {
        assert_eq!(
            vs_code_args("/p", None, 1),
            ["--reuse-window", "--", "/p"].map(OsString::from)
        );
        assert_eq!(
            vs_code_args("/p", Some("/p/a b.txt"), 12),
            ["--reuse-window", "/p", "--goto", "/p/a b.txt:12"].map(OsString::from)
        );
    }

    #[tokio::test]
    async fn the_launcher_receives_exact_arguments_and_its_failure_is_reported() {
        let root = folder("launch");
        let trace = root.join("trace");
        let script = root.join("opener");
        // Run through sh: executing a file just written can meet ETXTBSY while another test thread forks.
        let shell = Path::new("/bin/sh");
        program(
            &script,
            &format!("printf '%s|' \"$@\" > '{}'\n", trace.display()),
            0o600,
        );
        launch(
            shell,
            &[script.clone().into(), "one two".into(), "--x".into()],
            "failed",
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&trace).unwrap(), "one two|--x|");
        program(&script, "exit 3\n", 0o600);
        assert_eq!(
            launch(shell, &[script.clone().into()], "failed").await,
            Err("failed".to_owned())
        );
        assert_eq!(
            launch(&root.join("absent"), &[], "failed").await,
            Err("failed".to_owned())
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
