//! Pinned bubblewrap bind sources for explicitly selected filesystem views,
//! and the prepared view ([`Confinement`]) a confined engine or helper starts in.
//! Callers choose and validate grants; this module never interprets argv or PATH.
use std::{
    collections::BTreeMap,
    ffi::{CString, OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{fs::OpenOptionsExt, process::CommandExt},
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Open an absolute, normalized path without following aliases at any component.
pub fn pin(path: &Path) -> io::Result<File> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(io::Error::other("invalid_mount_path"));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY)
        .open("/")?;
    for component in path.components().skip(1) {
        let name = CString::new(component.as_os_str().as_encoded_bytes())
            .map_err(|_| io::Error::other("invalid_mount_path"))?;
        let fd = unsafe {
            libc::openat(
                file.as_raw_fd(),
                name.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        file = unsafe { File::from_raw_fd(fd) };
        if file.metadata()?.file_type().is_symlink() {
            return Err(io::Error::other("mount_source_changed"));
        }
    }
    Ok(file)
}

/// Append one pinned bind. Sources must be canonical before this call; callers
/// must preserve the pin and pass it only to bubblewrap, not to the final payload.
pub fn bind(
    args: &mut Vec<OsString>,
    pins: &mut Vec<Arc<File>>,
    source: &Path,
    target: &Path,
    read_only: bool,
) -> io::Result<()> {
    if source == Path::new("/") {
        return Err(io::Error::other("invalid_mount_path"));
    }
    bind_pinned(args, pins, Arc::new(pin(source)?), target, read_only)
}

/// Bind an already validated source without resolving its name again.
pub fn bind_pinned(
    args: &mut Vec<OsString>,
    pins: &mut Vec<Arc<File>>,
    file: Arc<File>,
    target: &Path,
    read_only: bool,
) -> io::Result<()> {
    if target == Path::new("/")
        || !target.is_absolute()
        || target
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(io::Error::other("invalid_mount_path"));
    }
    args.extend([
        if read_only {
            "--ro-bind-fd"
        } else {
            "--bind-fd"
        }
        .into(),
        file.as_raw_fd().to_string().into(),
        target.as_os_str().into(),
    ]);
    pins.push(file);
    Ok(())
}

/// One prepared bubblewrap view for one engine or helper process: the pinned
/// bubblewrap program, its view arguments (everything before `--`), the pinned
/// bind sources those arguments name by descriptor, environment values the
/// view requires (for example `TMPDIR` inside it) and the directories it shows
/// at another path. There is no unconfined form: a caller that cannot prepare
/// a view does not start the process.
#[derive(Clone)]
pub struct Confinement {
    program: PathBuf,
    args: Vec<OsString>,
    pins: Vec<Arc<File>>,
    environment: BTreeMap<String, String>,
    mapped: Vec<(PathBuf, PathBuf)>,
}
impl Confinement {
    pub fn new(
        program: PathBuf,
        args: Vec<OsString>,
        pins: Vec<Arc<File>>,
        environment: BTreeMap<String, String>,
        mapped: Vec<(PathBuf, PathBuf)>,
    ) -> Self {
        Self {
            program,
            args,
            pins,
            environment,
            mapped,
        }
    }
    /// Values the confined process must receive instead of the host's.
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
    /// The view arguments, without the program and the final `--`.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }
    /// The path at which the confined process sees a host `path` inside one of
    /// the directories the view shows elsewhere; any other path is unchanged.
    pub fn view_path(&self, path: &Path) -> PathBuf {
        for (host, view) in &self.mapped {
            if let Ok(rest) = path.strip_prefix(host) {
                return view.join(rest);
            }
        }
        path.to_owned()
    }
    /// `bwrap <view> -- executable args...`. The pinned sources stay open in the
    /// command until it is spawned and are inherited only by that child, which
    /// also gets `PR_SET_NO_NEW_PRIVS`.
    pub fn command<I, S>(&self, executable: &Path, args: I) -> std::process::Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = std::process::Command::new(&self.program);
        command
            .args(&self.args)
            .arg("--")
            .arg(executable)
            .args(args);
        let pins = self.pins.clone();
        unsafe {
            command.pre_exec(move || {
                for file in &pins {
                    if libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command
    }
}

/// Service policy that prepares the view of one engine launch when it actually
/// starts, after its launch environment (including a prepared Codex launcher)
/// exists. `inputs` are additional host files the engine must read (its model
/// instructions, the prepared launcher).
pub trait Confiner: Send + Sync + 'static {
    fn confine(
        &self,
        spec: &crate::environment::LaunchSpec,
        environment: &BTreeMap<String, String>,
        inputs: &[PathBuf],
    ) -> io::Result<Confinement>;
}
