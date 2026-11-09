//! Handing a folder or file to another program. Every target comes from the service, never from
//! JavaScript, and only fixed program names are looked up.
use std::{
    ffi::{OsStr, OsString},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{process::Command, time::timeout};

/// VS Code's own launcher and the builds that take the same arguments, best first.
const VS_CODE: &[&str] = &["code", "code-insiders", "codium"];
/// A window started from the desktop can have a shorter PATH than a terminal, so the usual homes are also tried.
const USUAL_FOLDERS: &[&str] = &["/usr/bin", "/usr/local/bin", "/snap/bin", "/usr/share/code/bin"];
/// The freedesktop opener, then GLib's. Absolute, so a project cannot put its own first.
const SYSTEM_OPENERS: &[(&str, &[&str])] = &[("/usr/bin/xdg-open", &[]), ("/usr/bin/gio", &["open"])];
/// A launcher that keeps running has started; waiting longer would only report a working window as a failure.
const HANDOVER: Duration = Duration::from_secs(8);

fn runnable(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn find_vs_code(path: Option<&OsStr>, usual: &[&str]) -> Option<PathBuf> {
    let mut folders: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    folders.extend(usual.iter().map(PathBuf::from));
    VS_CODE.iter().find_map(|name| folders.iter().map(|folder| folder.join(name)).find(|candidate| runnable(candidate)))
}

fn find_vs_code_for_home(path: Option<&OsStr>, usual: &[&str], home: Option<&Path>) -> Option<PathBuf> {
    find_vs_code(path, usual).or_else(|| {
        let home = home?;
        let folders = [home.join(".local/bin"), home.join(".local/share/code/bin")];
        VS_CODE.iter().find_map(|name| folders.iter().map(|p| p.join(name)).find(|p| runnable(p)))
    })
}

pub fn vs_code() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    find_vs_code_for_home(std::env::var_os("PATH").as_deref(), USUAL_FOLDERS, home.as_deref())
}

pub fn find_system_opener(openers: &[(&'static str, &'static [&'static str])]) -> Option<(PathBuf, &'static [&'static str])> {
    openers.iter().map(|(path, args)| (PathBuf::from(path), *args)).find(|(path, _)| runnable(path))
}

pub fn system_opener() -> Option<(PathBuf, &'static [&'static str])> {
    find_system_opener(SYSTEM_OPENERS)
}

/// VS Code's arguments for a folder, or for a file at a line inside that folder.
pub fn vs_code_args(root: &str, file: Option<&str>, line: u64) -> Vec<OsString> {
    match file {
        None => ["--reuse-window", "--", root].map(OsString::from).to_vec(),
        Some(file) => vec!["--reuse-window".into(), root.into(), "--goto".into(), format!("{file}:{line}").into()],
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
    if !runnable(terminal) { return Err("Native login needs gnome-terminal on this machine. No login was started.".into()); }
    let mut command: Vec<OsString> = vec!["--".into(), program.into()];
    command.extend_from_slice(args);
    launch(terminal, &command, "The native login terminal could not open.").await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cw-opener-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn program(path: &Path, body: &str, mode: u32) {
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
        assert_eq!(find_vs_code(Some(on_path.as_os_str()), &[&usual_text]), None);
        program(&on_path.join("code"), "", 0o600);
        assert_eq!(find_vs_code(Some(on_path.as_os_str()), &[&usual_text]), None, "a plain file named code is not an editor");
        program(&usual.join("codium"), "", 0o700);
        assert_eq!(find_vs_code(Some(on_path.as_os_str()), &[&usual_text]), Some(usual.join("codium")));
        program(&on_path.join("code"), "", 0o700);
        assert_eq!(find_vs_code(Some(on_path.as_os_str()), &[&usual_text]), Some(on_path.join("code")));
        assert_eq!(find_vs_code(None, &[&usual_text]), Some(usual.join("codium")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn desktop_finds_user_launcher_even_with_a_short_path() {
        let root = folder("home");
        let bin = root.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        program(&bin.join("code"), "", 0o700);
        assert_eq!(find_vs_code_for_home(None, &[], Some(&root)), Some(bin.join("code")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_first_runnable_system_opener_wins_and_none_is_an_answer() {
        let root = folder("system");
        let absent: &'static str = Box::leak(root.join("absent").to_str().unwrap().to_owned().into_boxed_str());
        let present: &'static str = Box::leak(root.join("gio").to_str().unwrap().to_owned().into_boxed_str());
        assert!(find_system_opener(&[(absent, &[])]).is_none());
        program(Path::new(present), "", 0o700);
        assert_eq!(find_system_opener(&[(absent, &[]), (present, &["open"])]), Some((PathBuf::from(present), &["open"][..])));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_folder_ends_the_options_and_a_file_opens_at_its_line() {
        assert_eq!(vs_code_args("/p", None, 1), ["--reuse-window", "--", "/p"].map(OsString::from));
        assert_eq!(vs_code_args("/p", Some("/p/a b.txt"), 12), ["--reuse-window", "/p", "--goto", "/p/a b.txt:12"].map(OsString::from));
    }

    #[tokio::test]
    async fn the_launcher_receives_exact_arguments_and_its_failure_is_reported() {
        let root = folder("launch");
        let trace = root.join("trace");
        let script = root.join("opener");
        // Run through sh: executing a file just written can meet ETXTBSY while another test thread forks.
        let shell = Path::new("/bin/sh");
        program(&script, &format!("printf '%s|' \"$@\" > '{}'\n", trace.display()), 0o600);
        launch(shell, &[script.clone().into(), "one two".into(), "--x".into()], "failed").await.unwrap();
        assert_eq!(std::fs::read_to_string(&trace).unwrap(), "one two|--x|");
        program(&script, "exit 3\n", 0o600);
        assert_eq!(launch(shell, &[script.clone().into()], "failed").await, Err("failed".to_owned()));
        assert_eq!(launch(&root.join("absent"), &[], "failed").await, Err("failed".to_owned()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
