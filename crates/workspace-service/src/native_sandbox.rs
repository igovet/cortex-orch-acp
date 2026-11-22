//! External MCP data-access boundary (Landlock ABI >=3 and seccomp).
//! Brokered command filesystem/process views live in native_command_sandbox.
//! Landlock alone does not deny chmod or supply private mounts.
use std::{
    ffi::CString,
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
};
const READ: u64 = (1 << 0) | (1 << 2) | (1 << 3);
#[cfg(test)]
#[test]
fn network_socket_is_denied_in_restricted_child() {
    let root = std::env::temp_dir().join(format!("native-sandbox-network-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let sandbox = match Sandbox::prepare(&root) {
        Ok(s) => s,
        Err(e) => {
            assert_eq!(e.to_string(), "native_verification_sandbox_unavailable");
            std::fs::remove_dir(&root).unwrap();
            return;
        }
    };
    // Child uses only async-signal-safe syscalls; no application/test locks.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        if unsafe { sandbox.enter() }.is_err() {
            unsafe { libc::_exit(77) };
        }
        let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        unsafe {
            libc::_exit(if socket < 0 { 0 } else { 1 });
        }
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    std::fs::remove_dir(&root).unwrap();
    assert!(libc::WIFEXITED(status));
    assert!(matches!(libc::WEXITSTATUS(status), 0 | 77));
}
#[cfg(test)]
#[test]
fn owner_unreadable_project_entries_are_skipped_not_refused() {
    use std::os::unix::fs::PermissionsExt;
    let base =
        std::env::temp_dir().join(format!("native-sandbox-unreadable-{}", std::process::id()));
    let (root, runtime) = (base.join("project"), base.join("runtime"));
    std::fs::create_dir_all(root.join("locked/inner")).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(root.join("readable.txt"), "ok").unwrap();
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    // Running as root would bypass the mode; then there is nothing to observe.
    let observable = std::fs::read_dir(root.join("locked")).is_err();
    let result = Sandbox::external(&root, &runtime, &[], &[], false);
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir_all(&base).unwrap();
    match result {
        Ok(_) => {}
        Err(e) if e.to_string() == "native_verification_sandbox_unavailable" => {}
        Err(e) => panic!("unreadable entry refused the server (observable={observable}): {e}"),
    }
}
#[repr(C)]
struct Ruleset {
    fs: u64,
}
#[repr(C, packed)]
struct PathRule {
    access: u64,
    parent: i32,
}
pub struct Sandbox {
    rules: OwnedFd,
    network: bool,
}
#[cfg(test)]
thread_local! {
    /// Per-thread count of external project walks, observed by unit tests only.
    pub(crate) static WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn error() -> io::Error {
    io::Error::last_os_error()
}
impl Sandbox {
    #[cfg(test)]
    pub fn prepare(root: &Path) -> io::Result<Self> {
        Self::prepare_rules(root)
            .map_err(|_| io::Error::other("native_verification_sandbox_unavailable"))
    }
    fn prepare_rules(root: &Path) -> io::Result<Self> {
        // Query, never assume the running host supports the installed headers.
        let abi = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, 0, 0, 1) };
        if abi < 3 {
            return Err(io::Error::other("native_verification_sandbox_unavailable"));
        }
        let rules = Ruleset { fs: (1 << 15) - 1 };
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &rules,
                std::mem::size_of::<Ruleset>(),
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::other("native_verification_sandbox_unavailable"));
        }
        let s = Self {
            rules: unsafe { OwnedFd::from_raw_fd(fd as i32) },
            network: false,
        };
        for path in [
            "/usr",
            "/bin",
            "/lib",
            "/lib64",
            "/etc/ld.so.cache",
            "/etc/alternatives",
            "/etc/localtime",
        ] {
            let p = Path::new(path);
            if p.exists() {
                s.allow(p, READ)?;
            }
        }
        // Directory enumeration is not recursive file-read permission.
        s.allow(root, 1 << 3)?;
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if [
                ".cortex",
                ".cortex-dev",
                ".git",
                ".cortex-dev-owner.json",
                ".cortex-workspace-dev.json",
            ]
            .iter()
            .any(|p| entry.file_name() == *p)
            {
                continue;
            }
            if !entry.file_type()?.is_symlink() {
                s.allow(&entry.path(), READ)?;
            }
        }
        s.allow(Path::new("/dev/null"), (1 << 1) | (1 << 2))?;
        Ok(s)
    }
    /// Explicit administrator grants plus the worker's reserved project scope.
    /// No mount namespaces or hostile-code-isolation claim.
    pub fn external(
        root: &Path,
        runtime: &Path,
        reads: &[std::path::PathBuf],
        writes: &[std::path::PathBuf],
        network: bool,
    ) -> io::Result<Self> {
        #[cfg(test)]
        WALKS.with(|w| w.set(w.get() + 1));
        let mut s = Self::prepare_rules(runtime)?;
        s.network = network;
        fn tree(s: &Sandbox, p: &Path) -> io::Result<()> {
            if p.file_name().is_some_and(|n| {
                [
                    ".git",
                    ".cortex",
                    ".cortex-dev",
                    ".codex",
                    ".claude",
                    ".ssh",
                    ".gnupg",
                    ".cortex-dev-owner.json",
                    ".cortex-workspace-dev.json",
                ]
                .iter()
                .any(|x| n == *x)
            }) {
                return Ok(());
            }
            // An entry the owner's own permissions already deny (for example a
            // root-owned 0700 build directory) gets no rule: the server could
            // not read it anyway, so skipping it grants nothing and does not
            // refuse the whole server. An entry removed while the walk runs has
            // no object left to grant. Any other error still fails closed.
            fn denied<T>(r: io::Result<T>) -> io::Result<Option<T>> {
                match r {
                    Ok(v) => Ok(Some(v)),
                    Err(e) if matches!(e.raw_os_error(), Some(libc::EACCES | libc::ENOENT)) => {
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            }
            let Some(m) = denied(p.symlink_metadata())? else {
                return Ok(());
            };
            if m.file_type().is_symlink() {
                return Ok(());
            }
            if m.is_dir() {
                let Some(entries) = denied(std::fs::read_dir(p))? else {
                    return Ok(());
                };
                if denied(s.allow(p, 1 << 3))?.is_none() {
                    return Ok(());
                }
                for e in entries {
                    tree(s, &e?.path())?;
                }
            } else if m.is_file() {
                denied(s.allow(p, READ))?;
            }
            Ok(())
        }
        tree(&s, root)?;
        for p in reads {
            s.allow(p, READ)?;
        }
        for p in writes.iter().chain(std::iter::once(&runtime.to_path_buf())) {
            let access = if p.is_dir() {
                (1 << 15) - 1
            } else {
                (1 << 0) | (1 << 1) | (1 << 2) | (1 << 14)
            };
            s.allow(p, access)?;
        }
        s.allow(Path::new("/dev/urandom"), 1 << 2)?;
        Ok(s)
    }
    fn allow(&self, path: &Path, mut access: u64) -> io::Result<()> {
        let c = CString::new(path.as_os_str().as_bytes())?;
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_dir() {
            access &= !(1 << 3);
        }
        let rule = PathRule {
            access,
            parent: file.as_raw_fd(),
        };
        if unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                self.rules.as_raw_fd(),
                1,
                &rule,
                0,
            )
        } < 0
        {
            return Err(error());
        }
        Ok(())
    }
    /// Only called in Command::pre_exec. No allocation or locks after fork.
    pub unsafe fn enter(&self) -> io::Result<()> {
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(error());
        }
        if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, self.rules.as_raw_fd(), 0) } < 0
        {
            return Err(error());
        }
        let (mut filters, n) = syscall_filter(self.network);
        let program = libc::sock_fprog {
            len: n as u16,
            filter: filters.as_mut_ptr(),
        };
        if unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2, &program) } != 0 {
            return Err(error());
        }
        Ok(())
    }
}

// Fixed-size stack program, also loaded by bubblewrap after namespace setup.
pub(super) fn syscall_filter(network: bool) -> ([libc::sock_filter; 64], usize) {
    // Deny network, process inspection/control, alternative filesystem IO and
    // namespace/mount control. Trusted local checks, not hostile-code isolation.
    const DENIED: &[libc::c_long] = &[
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
        libc::SYS_setsid,
        libc::SYS_setpgid,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_send_signal,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_io_uring_setup,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
    ];
    // Fixed-size stack BPF program; deny wrong architectures before matching
    // syscall numbers (including x32's alternate syscall-number namespace).
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xc00000b7;
    let mut filters = [libc::sock_filter {
        code: 0,
        jt: 0,
        jf: 0,
        k: 0,
    }; 64];
    let mut n = 0;
    macro_rules! push {
        ($code:expr,$jt:expr,$jf:expr,$k:expr) => {{
            filters[n] = libc::sock_filter {
                code: $code,
                jt: $jt,
                jf: $jf,
                k: $k,
            };
            n += 1;
        }};
    }
    push!(0x20, 0, 0, 4);
    push!(0x15, 1, 0, ARCH);
    push!(0x06, 0, 0, 0x80000000);
    push!(0x20, 0, 0, 0);
    push!(0x35, 0, 1, 0x40000000);
    push!(0x06, 0, 0, 0x80000000);
    for syscall in DENIED {
        if network && (*syscall == libc::SYS_socket || *syscall == libc::SYS_socketpair) {
            continue;
        }
        push!(0x15, 0, 1, *syscall as u32);
        push!(0x06, 0, 0, 0x00050000 | libc::EPERM as u32);
    }
    push!(0x06, 0, 0, 0x7fff0000);
    (filters, n)
}
