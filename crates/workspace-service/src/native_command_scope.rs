//! Memory isolation for supervised commands. Every command supervisor runs in its
//! own transient systemd user scope, a sibling of the service's cgroup, so a runaway
//! command is throttled or OOM-killed alone and never starves the service. Limits are
//! generous fractions of the machine's RAM. Without a usable `systemd-run` the
//! supervisor launches directly (previous behavior).
use std::{path::Path, process::Command, sync::OnceLock};

const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
/// Reclaim pressure starts here: 60% of RAM.
const HIGH_PERCENT: u64 = 60;
/// Hard kill limit for the command alone: 75% of RAM.
const MAX_PERCENT: u64 = 75;
/// Commands yield CPU to the service (default weight 100) and run at nice 10.
const COMMAND_CPU_WEIGHT: u32 = 20;
const COMMAND_NICE: i32 = 10;

fn mem_total() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = text
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kb * 1024)
}

/// The `systemd-run` arguments (before `--`) for one command scope.
pub(crate) fn scope_args(mem_total: u64, service_unit: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "--user", "--scope", "--collect", "--quiet",
        // The supervisor must survive its payload being OOM-killed to report it.
        "-p", "OOMPolicy=continue",
    ]
    .map(String::from)
    .to_vec();
    // Under contention the service and the desktop get the CPU before any command.
    args.push("-p".into());
    args.push(format!("CPUWeight={COMMAND_CPU_WEIGHT}"));
    args.push(format!("--nice={COMMAND_NICE}"));
    args.push("-p".into());
    args.push(format!("MemoryHigh={}", mem_total / 100 * HIGH_PERCENT));
    args.push("-p".into());
    args.push(format!("MemoryMax={}", mem_total / 100 * MAX_PERCENT));
    if let Some(unit) = service_unit {
        // Commands still end with the service, as they did inside its cgroup.
        args.push("-p".into());
        args.push(format!("BindsTo={unit}"));
        args.push("-p".into());
        args.push(format!("After={unit}"));
    }
    args
}

fn own_cgroup() -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    text.lines().find_map(|l| l.strip_prefix("0::")).map(str::to_owned)
}

fn service_unit() -> Option<String> {
    let path = own_cgroup()?;
    let leaf = path.rsplit('/').next()?;
    (leaf.ends_with(".service") && !leaf.contains(['\'', ' '])).then(|| leaf.to_owned())
}

fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Path::new(SYSTEMD_RUN).exists()
            && std::env::var_os("XDG_RUNTIME_DIR").is_some()
            && Command::new(SYSTEMD_RUN)
                .args(["--user", "--scope", "--collect", "--quiet", "--", "/bin/true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    })
}

/// The launcher for the supervisor: scoped when possible, otherwise the plain binary.
pub(crate) fn launcher(exe: &Path) -> Command {
    launcher_with(exe, available().then(mem_total).flatten(), service_unit())
}

fn launcher_with(exe: &Path, memory: Option<u64>, unit: Option<String>) -> Command {
    let mut command = launcher_inner(exe, memory, unit);
    lower_command_priority(&mut command);
    command
}

/// The supervisor and everything it starts get the idle disk class (inherited across
/// exec), so the service's own store and owner reads, which run at the session default,
/// never queue behind a build or test. The direct fallback also gets nice 10.
fn lower_command_priority(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    const IDLE: libc::c_long = 3 << 13;
    // SAFETY: only async-signal-safe syscalls (ioprio_set, setpriority) between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::syscall(libc::SYS_ioprio_set, 1, 0, IDLE);
            libc::setpriority(libc::PRIO_PROCESS, 0, COMMAND_NICE);
            Ok(())
        });
    }
}

fn launcher_inner(exe: &Path, memory: Option<u64>, unit: Option<String>) -> Command {
    match memory {
        Some(total) => {
            let mut command = Command::new(SYSTEMD_RUN);
            command.args(scope_args(total, unit.as_deref())).arg("--").arg(exe);
            command
        }
        None => Command::new(exe),
    }
}

/// After a scoped command ends: `Some(peak bytes)` when the kernel OOM-killed a
/// process in this command's own cgroup.
pub(crate) fn oom_killed() -> Option<u64> {
    let dir = Path::new("/sys/fs/cgroup").join(own_cgroup()?.trim_start_matches('/'));
    if !dir.to_string_lossy().contains(".scope") {
        return None;
    }
    let events = std::fs::read_to_string(dir.join("memory.events")).ok()?;
    let kills = events
        .lines()
        .find_map(|l| l.strip_prefix("oom_kill "))?
        .trim()
        .parse::<u64>()
        .ok()?;
    if kills == 0 {
        return None;
    }
    let peak = std::fs::read_to_string(dir.join("memory.peak"))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok());
    Some(peak.or_else(|| mem_total().map(|t| t / 100 * MAX_PERCENT)).unwrap_or(0))
}

pub(crate) fn oom_message(bytes: u64) -> String {
    format!(
        "\nCortex: command out of memory ({:.1} GB); it was stopped alone, the task continues.\n",
        bytes as f64 / 1e9
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_limits_are_generous_and_bound_to_the_service() {
        let args = scope_args(100_000_000_000, Some("cortex-workspace.service")).join(" ");
        assert!(args.contains("--user --scope --collect"));
        assert!(args.contains("MemoryHigh=60000000000"));
        assert!(args.contains("MemoryMax=75000000000"));
        assert!(args.contains("OOMPolicy=continue"));
        assert!(args.contains("CPUWeight=20") && args.contains("--nice=10"));
        assert!(args.contains("BindsTo=cortex-workspace.service"));
        assert!(!scope_args(1 << 30, None).join(" ").contains("BindsTo"));
    }

    #[test]
    fn launcher_wraps_in_a_scope_or_falls_back_to_direct_launch() {
        let exe = Path::new("/usr/bin/cortex-workspace-service");
        let scoped = launcher_with(exe, Some(1 << 33), None);
        assert_eq!(scoped.get_program(), SYSTEMD_RUN);
        let args: Vec<_> = scoped.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args.last().map(String::as_str), exe.to_str());
        assert!(args.iter().any(|a| a == "--"));
        let direct = launcher_with(exe, None, None);
        assert_eq!(direct.get_program(), exe);
        assert_eq!(direct.get_args().count(), 0);
    }

    #[test]
    fn oom_message_names_the_size() {
        assert!(oom_message(14_000_000_000).contains("out of memory (14.0 GB)"));
    }
}
