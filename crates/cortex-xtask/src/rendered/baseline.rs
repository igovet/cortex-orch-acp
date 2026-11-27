//! Opt-in measurement scope: the first X11 lifecycle window of a disposable run.

use super::journey::{outcome_pairs, J};
use super::session::{outcome, stop, Stack};
use crate::installed::util::*;
use crate::json::{parse, Res};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub(super) const SCRIPT: &str = include_str!("baseline.mjs");
pub(super) const FRAME: &str = include_str!("baseline-frame.c");
const PEER_HOOK: &str = include_str!("peer-timing.c");

pub(super) fn compile_peer_hook(directory: &Path, uid: u32, gid: u32) -> Res<()> {
    write(&directory.join("peer-timing.c"), PEER_HOOK)?;
    let done = capture(
        Command::new("docker")
            .args([
                "run",
                "--rm",
                "--pull=never",
                "--network=none",
                "--cap-drop=ALL",
                "--user",
            ])
            .arg(format!("{uid}:{gid}"))
            .arg("--mount")
            .arg(format!(
                "type=bind,source={},target=/out",
                directory.display()
            ))
            .args([
                "--entrypoint",
                "/usr/bin/cc",
                super::BUILDER,
                "-shared",
                "-fPIC",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-o",
                "/out/peer-timing.so",
                "/out/peer-timing.c",
                "-ldl",
            ]),
        std::time::Duration::from_secs(60),
    )?;
    require(
        done.code == 0,
        &format!("compile peer timing hook: {}", tail(&done.stderr)),
    )
}

pub(super) fn timed_peer(scratch: &Path, checkout: &Path, peer: &Path) -> Res<PathBuf> {
    let hook = Path::new("/harness/peer-timing.so");
    require(
        hook.is_file() && peer == Path::new("/artifacts/native-fake-engine"),
        "peer timing needs the private container and mounted fixture artifacts",
    )?;
    for name in [".cortex", ".cortex-dev"] {
        match std::fs::read_dir(checkout.join(name)) {
            Ok(mut entries) => require(
                entries.next().is_none(),
                "peer timing checkout store mask is not empty",
            )?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("peer timing checkout store mask: {e}")),
        }
    }
    // Every non-loopback network interface is excluded in the private container.
    let interfaces = std::fs::read_dir("/sys/class/net").map_err(|e| e.to_string())?;
    let interfaces = interfaces
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    require(
        interfaces.len() == 1 && interfaces[0].file_name() == "lo",
        "peer timing requires a loopback-only network namespace",
    )?;
    let trace = scratch.join("peer.trace");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&trace)
        .map_err(|e| e.to_string())?;
    let wrapper = scratch.join("timed-peer");
    write(
        &wrapper,
        "#!/bin/sh\nCORTEX_FIXTURE_PEER_TRACE=\"${0%/*}/peer.trace\" LD_PRELOAD=/harness/peer-timing.so exec /artifacts/native-fake-engine \"$@\"\n",
    )?;
    chmod(&wrapper, 0o700)?;
    Ok(wrapper)
}

pub(super) struct Authority {
    pub seed: PathBuf,
    path: PathBuf,
    cookie: String,
}

impl Authority {
    pub fn new(scratch: &Path) -> Res<Self> {
        let mut cookie = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut cookie))
            .map_err(|e| format!("X11 cookie: {e}"))?;
        let authority = Self {
            seed: scratch.join("seed.auth"),
            path: scratch.join("client.auth"),
            cookie: cookie.iter().map(|byte| format!("{byte:02x}")).collect(),
        };
        authority.record(&authority.seed, ":0")?;
        Ok(authority)
    }

    fn record(&self, file: &Path, display: &str) -> Res<()> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(file)
            .map_err(|e| e.to_string())?;
        let mut child = Command::new("xauth")
            .arg("-q")
            .arg("-f")
            .arg(file)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("xauth: {e}"))?;
        let written = writeln!(
            child.stdin.take().unwrap(),
            "add {display} MIT-MAGIC-COOKIE-1 {}",
            self.cookie
        );
        let status = child.wait().map_err(|e| format!("xauth: {e}"))?;
        written.map_err(|e| format!("xauth input: {e}"))?;
        require(
            status.success(),
            "xauth could not write private authorization",
        )?;
        require(
            file.metadata().map_err(|e| e.to_string())?.len() > 0,
            "private Xauthority is empty",
        )
    }

    pub fn client(self, display: &str) -> Res<PathBuf> {
        require(
            display
                .strip_prefix(':')
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())),
            "private Xvfb returned an invalid display",
        )?;
        self.record(&self.path, display)?;
        Ok(self.path)
    }
}

pub(super) fn verify_authority(scratch: &Path) -> Res<()> {
    let empty = scratch.join("no-cookie");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&empty)
        .map_err(|e| e.to_string())?;
    let rejected = capture(
        Command::new("xdpyinfo").env("XAUTHORITY", empty),
        std::time::Duration::from_secs(10),
    )?;
    run_ok(&["xdpyinfo"])?;
    require(
        rejected.code == 1 && rejected.stdout.is_empty(),
        &format!(
            "private Xvfb no-cookie probe expected exit 1; observed {}",
            rejected.code
        ),
    )
}

pub(super) struct Observer {
    pub runtime: PathBuf,
    directory: PathBuf,
    child: Option<Child>,
}

impl Observer {
    pub fn start(st: &Stack) -> Res<Self> {
        let short = st.scratch.join("observed");
        let runtime = short.join("run");
        let endpoint = runtime.join(st.ns("runtime_namespace"));
        std::fs::create_dir_all(&endpoint).map_err(|e| e.to_string())?;
        for path in [&short, &runtime, &endpoint] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let script = short.join("baseline.mjs");
        write(&script, SCRIPT)?;
        let mut command = Command::new("node");
        command
            .arg(script)
            .arg(&short)
            .arg(st.socket())
            .arg(endpoint.join("service.sock"));
        if st.peer_timing {
            command.arg(st.scratch.join("peer.trace"));
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("start rendered observer: {e}"))?;
        let mut observer = Self {
            runtime,
            directory: short,
            child: Some(child),
        };
        observer.wait("ready", 15)?;
        Ok(observer)
    }

    pub fn diagnostic(&self) -> Option<String> {
        read_to_string(&self.directory.join("failure")).ok()
    }

    fn wait(&mut self, name: &str, seconds: u64) -> Res<()> {
        wait_secs(&format!("rendered observer {name}"), seconds, || {
            let failure = self.directory.join("failure");
            if failure.is_file() {
                return Err(format!("rendered observer: {}", read_to_string(&failure)?));
            }
            if self
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err("rendered observer exited before delivery".into());
            }
            Ok(self.directory.join(name).is_file())
        })
    }

    fn signal(&self, name: &str, contents: &str) -> Res<()> {
        let pending = self.directory.join(format!("{name}.pending"));
        write(&pending, contents)?;
        std::fs::rename(pending, self.directory.join(name)).map_err(|e| e.to_string())
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        stop(&mut self.child);
    }
}

pub(super) fn begin(j: &mut J) -> Res<()> {
    if j.st.observer.is_none() {
        return Ok(());
    }
    let window = j.st.window_id()?;
    let route = j.path()?;
    let task = route
        .rsplit('/')
        .next()
        .filter(|id| id.starts_with("wt_"))
        .ok_or("missing baseline task identity")?;
    let observer = j.st.observer.as_mut().unwrap();
    observer.signal(
        "begin",
        &line(&obj(vec![("window", s(window)), ("taskId", s(task))])),
    )?;
    observer.wait("started", 15)
}

fn peer_binding(j: &mut J) -> Res<String> {
    let route = j.path()?;
    let task = route.rsplit('/').next().unwrap_or_default();
    require(
        task.len() == 35
            && task.starts_with("wt_")
            && task[3..].bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid fixture task binding",
    )?;
    // Query only the disposable project, read-only, after the real question appeared.
    let db = j.channel.store_db(&j.home.join("journey"));
    let binding = crate::installed::native::query(
        &db,
        &format!(
            "SELECT json_group_array(json_object('taskId', w.task_id, 'projectId', w.project_id,
            'runtimeTaskId', e.runtime_task_id, 'attemptId', d.attempt_id,
            'sessionId', d.engine_session_ref, 'displaySessionId', h.session_id,
            'sourceSequence', h.seq))
         FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id
         JOIN delegation_provisioning d ON d.task_id=e.runtime_task_id
         JOIN host_transcript h ON h.session_id=e.acp_session_id AND h.kind='agent_message'
           AND h.text='Post-question coordinator text'
           AND substr(h.message_id, 1, length(d.attempt_id)+1)=d.attempt_id||':'
         WHERE w.task_id='{task}' AND d.role_id='orchestrator'
           AND (SELECT count(*) FROM workspace_executions)=1
           AND (SELECT count(*) FROM runtime_tasks)=1"
        ),
    )?;
    Ok(binding)
}

pub(super) fn finish(j: &mut J) -> Res<()> {
    if j.st.observer.is_none() {
        return Ok(());
    }
    // Record the settled task view as well as the active Start→question journey.
    sleep_ms(2000);
    let binding = if j.st.peer_timing {
        peer_binding(j)?
    } else {
        String::new()
    };
    let observer = j.st.observer.as_mut().unwrap();
    observer.signal("finish", &binding)?;
    observer.wait("result", 60)?;
    let mut evidence = parse(&read_to_string(&observer.directory.join("result"))?)?;
    evidence.set(
        "displayAuthorization",
        s("MIT-MAGIC-COOKIE-1; authenticated and no-cookie refusal observed"),
    );
    evidence.set(
        "desktopSha256",
        s(sha_file(&j.st.artifacts.join(j.st.ns("desktop_binary")))?),
    );
    evidence.set(
        "serviceSha256",
        s(sha_file(&j.st.artifacts.join(j.st.ns("service_binary")))?),
    );
    evidence.set(
        "peerSha256",
        s(sha_file(&j.st.artifacts.join("native-fake-engine"))?),
    );
    evidence.set(
        "harnessSha256",
        s(sha_file(
            &std::env::current_exe().map_err(|e| e.to_string())?,
        )?),
    );
    outcome(
        &mut j.run,
        "rendered_baseline",
        "passed",
        outcome_pairs(&evidence),
    );
    Ok(())
}
