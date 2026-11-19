//! Host effects. Never accepts a model's project digest or command pass flag.
use crate::{
    machine::files::revision,
    native_command_sandbox::{Profile, Sandbox},
};
use cortex_core::{lifecycle::completion::EvidenceSnapshot, CoreError, CoreResult};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::File,
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
const OUTPUT: usize = 1024 * 1024;
/// Bounds of one Git listing used to observe the project (its paths, index, ignored
/// directories and ignored files). A dependency or build folder can hold most of a
/// project's files, so these are far above `OUTPUT`; a listing past either bound is
/// reported (`project_unobservable_too_many_paths`, or unavailable), never truncated.
const LISTING_OUTPUT: usize = 256 * 1024 * 1024;
const LISTING_DEADLINE: Duration = Duration::from_secs(120);
// Leave startup/cleanup margin above a 900-second bounded build or test step.
const VERIFICATION_DEADLINE: Duration = Duration::from_secs(20 * 60);
pub(super) fn identity() -> CoreResult<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(revision(&bytes))
}
fn refused() -> CoreError {
    CoreError::Refused("native_effect_unavailable")
}
pub(super) struct Output {
    pub code: Option<i32>,
    pub complete: bool,
    pub truncated: bool,
    failure: Option<&'static str>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
async fn drain(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut truncated = false;
    loop {
        let n = stream.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        let take = n.min(limit - kept.len());
        kept.extend_from_slice(&buffer[..take]);
        truncated |= take < n;
    }
    Ok((kept, truncated))
}
async fn capture(
    cmd: Command,
    deadline: Duration,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> CoreResult<Output> {
    capture_limited(cmd, deadline, cancel, OUTPUT).await
}
/// `capture` keeping at most `limit` bytes of each stream (the rest is drained and
/// the output marked truncated, so incomplete).
async fn capture_limited(
    mut cmd: Command,
    deadline: Duration,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    limit: usize,
) -> CoreResult<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| {
        if e.raw_os_error() == Some(libc::ENOTSUP) {
            CoreError::Refused("native_verification_sandbox_unavailable")
        } else {
            refused()
        }
    })?;
    let pid = child.id().ok_or_else(refused)?;
    let out = tokio::spawn(drain(child.stdout.take().unwrap(), limit));
    let err = tokio::spawn(drain(child.stderr.take().unwrap(), OUTPUT.min(limit)));
    let status = tokio::select! {
        s = tokio::time::timeout(deadline, child.wait()) => match s {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(_)) => Err("command_wait_failed"),
            Err(_) => Err("command_timeout"),
        },
        _ = async { while !*cancel.borrow_and_update() { if cancel.changed().await.is_err() { std::future::pending::<()>().await; } } } => Err("owner_stopped"),
    };
    let (code, failure) = match status {
        Ok(status) => match status.code() {
            Some(code) => (Some(code), None),
            None => (None, Some("command_signalled")),
        },
        Err(reason) => {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            };
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            (None, Some(reason))
        }
    };
    // Restricted verifier descendants cannot escape their process group.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    };
    let mut out = out;
    let mut err = err;
    let drained = tokio::time::timeout(Duration::from_secs(1), async {
        Ok::<_, CoreError>((
            (&mut out)
                .await
                .map_err(|_| refused())?
                .map_err(|_| refused())?,
            (&mut err)
                .await
                .map_err(|_| refused())?
                .map_err(|_| refused())?,
        ))
    })
    .await;
    match drained {
        Ok(Ok(((stdout, a), (stderr, b)))) => Ok(Output {
            code,
            complete: code.is_some() && !a && !b,
            truncated: a || b,
            failure: failure.or((a || b).then_some("command_output_limit")),
            stdout,
            stderr,
        }),
        _ => {
            out.abort();
            err.abort();
            Ok(Output {
                code,
                complete: false,
                truncated: true,
                failure: failure.or(Some("command_output_unavailable")),
                stdout: vec![],
                stderr: vec![],
            })
        }
    }
}
fn command(root: &Path, argv: &[String]) -> Command {
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..])
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("LANG", "C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0");
    c
}
#[cfg(test)]
pub(super) async fn verify(root: PathBuf, argv: &[String]) -> CoreResult<Value> {
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    verify_cancelled(root, argv, receiver).await
}
pub(super) async fn verify_cancelled(
    root: PathBuf,
    argv: &[String],
    cancel: tokio::sync::watch::Receiver<bool>,
) -> CoreResult<Value> {
    verify_in(root, argv, cancel, None).await
}
/// The verifier launch. Production passes no environment: the sandbox takes the
/// service's own with `PATH` set to the owner's tool search path
/// (`crate::tool_path`); tests pass an explicit owner environment.
async fn verify_in(
    root: PathBuf,
    argv: &[String],
    cancel: tokio::sync::watch::Receiver<bool>,
    environment: Option<BTreeMap<OsString, OsString>>,
) -> CoreResult<Value> {
    let r = root.clone();
    let sandbox = tokio::task::spawn_blocking(move || {
        let ignored = crate::native_command_sandbox::ignored_directories(&r)?;
        match environment {
            None => Sandbox::prepare(&r, Profile::ReadOnly, &ignored, &[], false),
            Some(environment) => {
                Sandbox::with_environment(&r, Profile::ReadOnly, &ignored, &[], false, environment)
            }
        }
    })
    .await
    .map_err(|_| refused())?
    .map_err(|_| CoreError::Refused("native_verification_sandbox_unavailable"))?;
    let c = Command::from(sandbox.command(argv)?);
    let result = capture(c, VERIFICATION_DEADLINE, cancel).await;
    let receipt = match result {
        Ok(o) => json!({"complete":o.complete,"exit_code":o.code,"output_truncated":o.truncated,
            "failure":o.failure,"stdout":String::from_utf8_lossy(&o.stdout),"stderr":String::from_utf8_lossy(&o.stderr)}),
        Err(e) => {
            json!({"complete":false,"exit_code":null,"output_truncated":false,"failure":e.code()})
        }
    };
    Ok(
        json!({"tool_call_ref":format!("native-command:{}",identity()?),
        "argv":argv,"cwd":root,"profile":"native-readonly-bubblewrap","observation":receipt}),
    )
}
async fn git(root: &Path, args: &[&str]) -> CoreResult<Vec<u8>> {
    git_with_limit(root, args, LISTING_OUTPUT).await
}
async fn git_with_limit(root: &Path, args: &[&str], output_limit: usize) -> CoreResult<Vec<u8>> {
    let mut argv = vec![
        "/usr/bin/git".to_owned(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    let o = capture_limited(
        command(root, &argv),
        LISTING_DEADLINE,
        receiver,
        output_limit,
    )
    .await?;
    // A listing past its byte bound is reported as such: it never stands in, cut
    // short, for the whole project.
    if o.truncated && o.code == Some(0) {
        return Err(CoreError::Refused("project_unobservable_too_many_paths"));
    }
    if !o.complete || o.code != Some(0) {
        return Err(CoreError::Refused("project_revision_unavailable"));
    }
    Ok(o.stdout)
}
/// Availability probe is not retained as verification evidence.
pub(super) async fn plan_preflight(root: PathBuf, plan: &Value) -> CoreResult<()> {
    let canonical = cortex_core::lifecycle::plan::validate(plan)?;
    let tasks = canonical["tasks"].as_array().unwrap();
    fn depends(tasks: &[Value], id: &str, target: &str) -> bool {
        let mut visited = BTreeSet::new();
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let node = tasks.iter().find(|n| n["id"] == id).unwrap();
            for parent in node["dependencies"].as_array().unwrap() {
                let parent = parent.as_str().unwrap();
                if parent == target {
                    return true;
                }
                pending.push(parent);
            }
        }
        false
    }
    for n in tasks {
        if !cortex_core::scheduler::policy::read_only(n["role"].as_str().unwrap(), "execution") {
            continue;
        }
        for path in n["scope"].as_array().unwrap() {
            let path = path.as_str().unwrap();
            if path.contains(['*', '?', '[']) || root.join(path).exists() {
                continue;
            }
            let writer = tasks.iter().any(|w| {
                !cortex_core::scheduler::policy::read_only(w["role"].as_str().unwrap(), "execution")
                    && (depends(tasks, n["id"].as_str().unwrap(), w["id"].as_str().unwrap())
                        || depends(tasks, w["id"].as_str().unwrap(), n["id"].as_str().unwrap()))
                    && w["scope"].as_array().unwrap().iter().any(|s| {
                        cortex_core::scheduler::policy::overlaps(path, s.as_str().unwrap())
                    })
            });
            if !writer {
                return Err(CoreError::Refused("read_only_scope_not_writable"));
            }
        }
    }
    verification_preflight(root, &canonical["verification"]).await
}
pub(super) async fn verification_preflight(root: PathBuf, verification: &Value) -> CoreResult<()> {
    // The same environment the verifier's launch takes: the owner's tool search path.
    verification_preflight_in(root, verification, crate::tool_path::environment()).await
}
async fn verification_preflight_in(
    root: PathBuf,
    verification: &Value,
    environment: BTreeMap<OsString, OsString>,
) -> CoreResult<()> {
    for argv in cortex_core::lifecycle::plan::verification(verification)? {
        // A bare name resolves where the verifier's own launch resolves it: on the
        // sandbox's command search path (project tool directories, then the owner's
        // PATH), not a fixed system list. Relative search entries are not probed.
        let candidates: Vec<PathBuf> = if argv[0].contains('/') {
            vec![root.join(&argv[0])]
        } else {
            let path = crate::native_command_sandbox::command_path(&root, &environment)
                .unwrap_or_default();
            std::env::split_paths(&path)
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(&argv[0]))
                .filter(|p| p.is_file())
                .collect()
        };
        if candidates.is_empty() {
            return Err(CoreError::Refused("verification_executable_unavailable"));
        }
        let mut available = false;
        for executable in candidates {
            let (_cancel, receiver) = tokio::sync::watch::channel(false);
            let probe = verify_in(
                root.clone(),
                &[
                    "/usr/bin/test".into(),
                    "-x".into(),
                    executable.to_string_lossy().into(),
                ],
                receiver,
                Some(environment.clone()),
            )
            .await?;
            if probe["observation"]["failure"] == "native_verification_sandbox_unavailable" {
                return Err(CoreError::Refused(
                    "native_verification_sandbox_unavailable",
                ));
            }
            if probe["observation"]["complete"] == true && probe["observation"]["exit_code"] == 0 {
                available = true;
                break;
            }
        }
        if !available {
            return Err(CoreError::Refused("verification_executable_unavailable"));
        }
    }
    Ok(())
}
pub(super) async fn git_ref(root: &Path, args: &[&str]) -> CoreResult<Output> {
    let mut argv = vec![
        "/usr/bin/git".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    let result = capture(command(root, &argv), Duration::from_secs(10), receiver).await?;
    if !result.complete || !matches!(result.code, Some(0 | 1)) {
        return Err(CoreError::Refused("project_revision_unavailable"));
    }
    Ok(result)
}
struct CheckoutHead {
    commit: Option<String>,
    /// Preserve the project digest input, including the symbolic ref when present.
    digest_input: Vec<u8>,
}
async fn git_head(root: &Path) -> CoreResult<CheckoutHead> {
    let head = git_ref(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await?;
    let symbolic = git_ref(root, &["symbolic-ref", "--quiet", "HEAD"]).await?;
    if head.code == Some(1) {
        if symbolic.code != Some(0) {
            return Err(refused());
        }
        let branch = std::str::from_utf8(&symbolic.stdout)
            .map_err(|_| refused())?
            .trim_end_matches('\n');
        if !branch.starts_with("refs/") || branch.len() > 1024 {
            return Err(refused());
        }
        let missing = git_ref(root, &["show-ref", "--verify", "--quiet", branch]).await?;
        if missing.code != Some(1) {
            return Err(refused());
        }
    }
    let commit = if head.code == Some(0) {
        let oid = std::str::from_utf8(&head.stdout)
            .map_err(|_| refused())?
            .trim_end_matches('\n');
        if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(refused());
        }
        Some(oid.to_owned())
    } else {
        None
    };
    let mut digest_input = head.stdout;
    digest_input.push(0);
    digest_input.extend(symbolic.stdout);
    Ok(CheckoutHead {
        commit,
        digest_input,
    })
}
/// How completely one observation covers the project. A waiver of review is never
/// granted while any entry stands in by metadata only or the observation is incomplete.
#[derive(Default, Clone, Copy)]
struct Flags {
    metadata_only: bool,
    cap_reached: bool,
    incomplete: bool,
}
impl Flags {
    fn merge(&mut self, other: Flags) {
        self.metadata_only |= other.metadata_only;
        self.cap_reached |= other.cap_reached;
        self.incomplete |= other.incomplete;
    }
}
struct Observed {
    digest: String,
    head: Option<String>,
    ignored: Option<String>,
    flags: Flags,
    /// Per-file identity of the top-level checkout (only when asked for): a content
    /// hash per file, `dir:<commit>:<nested digest>` per nested checkout, and `empty/`
    /// per empty directory.
    manifest: std::collections::BTreeMap<String, String>,
    /// Raw content hashes for regular files fully read in this observation.
    raw_manifest: std::collections::BTreeMap<String, String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ManifestMode {
    None,
    Identity,
    Review,
}
const NESTED_DEPTH: u8 = 4;
pub(super) async fn observe(root: PathBuf) -> CoreResult<EvidenceSnapshot> {
    Ok(observe_with(root, ManifestMode::None).await?.0)
}
/// `observe`'s project digest alone, for presentation reads that compare a result with
/// the folder. Ignored files feed only the ignored digest and completeness flags, so
/// this pass never lists or visits them (a dependency folder can hold most of a
/// project's files); the digest is the one `observe` returns for the same folder.
pub(super) async fn observe_digest(root: PathBuf) -> CoreResult<String> {
    Ok(observe_checkout(root, 0, ManifestMode::None, false)
        .await?
        .digest)
}
/// Observation plus the per-file manifest of the same pass, for change attribution.
/// The manifest is `None` when the observation is incomplete or stands in by metadata
/// only anywhere: a changed-file list built from it could not be trusted.
pub(super) async fn observe_manifest(
    root: PathBuf,
) -> CoreResult<(
    EvidenceSnapshot,
    Option<std::collections::BTreeMap<String, String>>,
)> {
    let (snapshot, manifest, _) = observe_with(root, ManifestMode::Identity).await?;
    let usable = !(snapshot.incomplete || snapshot.metadata_only || snapshot.cap_reached);
    Ok((snapshot, usable.then_some(manifest)))
}
/// Observation used to prepare a reviewer packet. Raw SHA-256 values are
/// retained only for regular files whose complete bytes were read during the
/// same bounded observation.
pub(super) async fn observe_review_manifest(
    root: PathBuf,
) -> CoreResult<(
    EvidenceSnapshot,
    Option<std::collections::BTreeMap<String, String>>,
    std::collections::BTreeMap<String, String>,
)> {
    let (snapshot, manifest, raw) = observe_with(root, ManifestMode::Review).await?;
    let usable = !(snapshot.incomplete || snapshot.metadata_only || snapshot.cap_reached);
    Ok((snapshot, usable.then_some(manifest), raw))
}
async fn observe_with(
    root: PathBuf,
    with_manifest: ManifestMode,
) -> CoreResult<(
    EvidenceSnapshot,
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, String>,
)> {
    let o = observe_checkout(root, 0, with_manifest, true).await?;
    let manifest = o.manifest;
    let o = Observed {
        manifest: Default::default(),
        ..o
    };
    Ok((
        EvidenceSnapshot {
            project_digest: Some(o.digest),
            head: o.head,
            ignored_digest: o.ignored,
            metadata_only: o.flags.metadata_only,
            cap_reached: o.flags.cap_reached,
            incomplete: o.flags.incomplete,
            ..Default::default()
        },
        manifest,
        o.raw_manifest,
    ))
}
type ObservedFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = CoreResult<Observed>> + Send>>;
fn observe_root(root: PathBuf, depth: u8, with_ignored: bool) -> ObservedFuture {
    Box::pin(observe_checkout(
        root,
        depth,
        ManifestMode::None,
        with_ignored,
    ))
}
/// What one observation pass hashes: Git's listing of a repository, or the plain walk
/// of a folder that is in no repository (`plain_listing`).
struct Listing {
    /// NUL-separated deliverable paths; a nested checkout is listed as `dir/`.
    paths: Vec<u8>,
    index: Vec<u8>,
    pruned: Option<BTreeSet<Vec<u8>>>,
    ignored: Option<Vec<u8>>,
    head: CheckoutHead,
}
async fn observe_checkout(
    root: PathBuf,
    depth: u8,
    with_manifest: ManifestMode,
    with_ignored: bool,
) -> CoreResult<Observed> {
    // A plain project folder (in no Git repository at all) is observed by walking it.
    // Only Git's own "not a git repository" answer selects that walk: any other Git
    // failure (a broken, unreadable or untrusted repository) stays a refusal, and a
    // repository is always observed by Git, so its digest never depends on this.
    let listing = match git_listing(&root, with_ignored).await {
        Ok(listing) => listing,
        Err(e) if e.code() == "project_revision_unavailable" && no_repository(&root).await => {
            let r = root.clone();
            tokio::task::spawn_blocking(move || plain_listing(&r, with_ignored))
                .await
                .map_err(|_| refused())??
        }
        Err(e) => return Err(e),
    };
    observe_listed(root, depth, with_manifest, with_ignored, listing).await
}
/// True only when Git itself answers that `root` is in no repository (its own
/// "not a git repository" refusal, which also names a filesystem boundary) and `root`
/// holds no `.git` entry. A Git that cannot run, times out or fails otherwise is not
/// such an answer.
async fn no_repository(root: &Path) -> bool {
    if !matches!(std::fs::symlink_metadata(root.join(".git")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return false;
    }
    let argv = [
        "/usr/bin/git".to_owned(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "rev-parse".into(),
        "--is-inside-work-tree".into(),
    ];
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    match capture(command(root, &argv), Duration::from_secs(10), receiver).await {
        Ok(o) => {
            o.complete
                && o.code == Some(128)
                && o.stdout.is_empty()
                && String::from_utf8_lossy(&o.stderr).contains("not a git repository")
        }
        Err(_) => false,
    }
}
/// The private stores and dev markers at the top of a project; never project content.
const PRIVATE_TOP: [&str; 4] = [
    ".cortex",
    ".cortex-dev",
    ".cortex-dev-owner.json",
    ".cortex-workspace-dev.json",
];
/// The listing of a plain project folder, in no Git repository: every entry that is
/// not a directory (regular files, symlinks by their target, special files) is a
/// deliverable path, as Git lists an untracked one; a directory holding a `.git` entry
/// is a nested checkout listed as `dir/` and observed by its own pass; nothing is
/// ignored (no ignore rules apply without a repository), so the ignored listing is
/// empty. The private stores at the top and `.cortex`/`.cortex-dev` directories at any
/// depth are skipped, as the empty-directory walk skips them. An unreadable directory
/// is a refusal, never a partial listing; more than `WALK_LIMIT` entries is refused.
/// Order does not matter: the digest sorts the names.
fn plain_listing(root: &Path, with_ignored: bool) -> CoreResult<Listing> {
    use std::os::unix::ffi::OsStrExt;
    let mut paths = Vec::new();
    let mut seen = 0usize;
    let mut pending = vec![(root.to_owned(), Vec::<u8>::new())];
    while let Some((dir, rel)) = pending.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|_| unreadable())? {
            let entry = entry.map_err(|_| unreadable())?;
            seen += 1;
            if seen > WALK_LIMIT {
                return Err(CoreError::Refused("project_unobservable_too_many_paths"));
            }
            let name = entry.file_name();
            let name = name.as_bytes();
            if rel.is_empty() && PRIVATE_TOP.iter().any(|t| t.as_bytes() == name) {
                continue;
            }
            let mut path = rel.clone();
            if !path.is_empty() {
                path.push(b'/');
            }
            path.extend_from_slice(name);
            if entry.file_type().map_err(|_| unreadable())?.is_dir() {
                if name == b".cortex" || name == b".cortex-dev" {
                    continue;
                }
                if entry.path().join(".git").symlink_metadata().is_ok() {
                    path.push(b'/');
                    paths.extend_from_slice(&path);
                    paths.push(0);
                    continue;
                }
                pending.push((entry.path(), path));
            } else {
                paths.extend_from_slice(&path);
                paths.push(0);
            }
        }
    }
    Ok(Listing {
        paths,
        index: Vec::new(),
        pruned: None,
        ignored: with_ignored.then(Vec::new),
        head: CheckoutHead {
            commit: None,
            digest_input: b"plain-folder".to_vec(),
        },
    })
}
async fn git_listing(root: &Path, with_ignored: bool) -> CoreResult<Listing> {
    git_listing_with_ignored_limit(root, with_ignored.then_some(LISTING_OUTPUT)).await
}
async fn git_listing_with_ignored_limit(
    root: &Path,
    ignored_output_limit: Option<usize>,
) -> CoreResult<Listing> {
    let root = root.to_owned();
    const EXCLUDE: [&str; 4] = [
        ":(exclude).cortex",
        ":(exclude).cortex-dev",
        ":(exclude).cortex-dev-owner.json",
        ":(exclude).cortex-workspace-dev.json",
    ];
    let pathspec = |base: &[&'static str]| -> Vec<&'static str> {
        let mut v = base.to_vec();
        v.extend(["--", "."]);
        v.extend(EXCLUDE);
        v
    };
    let paths = git(
        &root,
        &pathspec(&[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ]),
    )
    .await?;
    let index = git(&root, &pathspec(&["ls-files", "--stage", "-z"])).await?;
    // The outermost wholly ignored directories (Git stops at each): the empty-directory
    // walk never descends into them, in every observation, so their digests agree.
    // A failed listing makes the folder unobservable; it never switches to another
    // walk rule, whose digest would differ for the same tree.
    let pruned: Option<BTreeSet<Vec<u8>>> = Some(
        git(
            &root,
            &pathspec(&[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ]),
        )
        .await?
        .split(|c| *c == 0)
        .filter_map(|p| p.strip_suffix(b"/"))
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect(),
    );
    // Spend a separate listing budget on ignored metadata. An overflow here
    // must not discard the deliverable listing or supply a partial digest.
    // A digest-only observation never requests this listing.
    let ignored = if let Some(limit) = ignored_output_limit {
        git_with_limit(
            &root,
            &pathspec(&[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
            ]),
            limit,
        )
        .await
        .ok()
    } else {
        None
    };
    // Ref-only queries never invoke worktree clean/smudge filters or hooks.
    let head = git_head(&root).await?;
    Ok(Listing {
        paths,
        index,
        pruned,
        ignored,
        head,
    })
}
async fn observe_listed(
    root: PathBuf,
    depth: u8,
    with_manifest: ManifestMode,
    with_ignored: bool,
    listing: Listing,
) -> CoreResult<Observed> {
    let Listing {
        paths,
        index,
        pruned,
        ignored,
        head,
    } = listing;
    // Submodules and nested repositories: the commit their checkout is at plus the
    // digest of everything inside the checkout (tracked changes, untracked files,
    // empty directories, its own ignored files), observed the same way as the project.
    let mut heads = std::collections::BTreeMap::new();
    let mut flags = Flags::default();
    let mut nested_ignored: Vec<(String, Option<String>)> = Vec::new();
    let mut linked: Vec<String> = index
        .split(|c| *c == 0)
        .filter(|e| e.starts_with(b"160000 "))
        .filter_map(|e| e.splitn(2, |c| *c == b'\t').nth(1))
        .filter_map(|p| std::str::from_utf8(p).ok().map(str::to_owned))
        .collect();
    linked.extend(
        paths
            .split(|c| *c == 0)
            .filter(|p| p.ends_with(b"/"))
            .filter_map(|p| std::str::from_utf8(p).ok())
            .map(|p| p.trim_end_matches('/').to_owned()),
    );
    linked.sort();
    linked.dedup();
    for path in linked {
        let checkout = root.join(&path);
        let sane = !path.starts_with('/') && !path.split('/').any(|c| c == ".." || c == ".git");
        if !sane {
            return Err(CoreError::Refused("project_unobservable_path"));
        }
        // An uninitialised submodule is an empty directory; never ask the parent repository.
        if std::fs::symlink_metadata(checkout.join(".git")).is_err() {
            continue;
        }
        let r = git_ref(&checkout, &["rev-parse", "--verify", "--quiet", "HEAD"]).await?;
        let mut value = if r.code == Some(0) {
            String::from_utf8_lossy(&r.stdout).trim().to_owned()
        } else {
            "unborn".to_owned()
        };
        if depth >= NESTED_DEPTH {
            flags.incomplete = true;
            value.push_str(":unobserved-depth");
        } else {
            match observe_root(checkout, depth + 1, with_ignored).await {
                Ok(nested) => {
                    flags.merge(nested.flags);
                    value.push(':');
                    value.push_str(&nested.digest);
                    // Ignored files are not deliverable content: they feed the ignored digest.
                    nested_ignored.push((path.clone(), nested.ignored));
                }
                Err(e)
                    if matches!(
                        e.code(),
                        "project_revision_unavailable" | "project_unobservable_too_many_paths"
                    ) =>
                {
                    flags.incomplete = true;
                    value.push_str(":unobserved");
                }
                Err(e) => return Err(e),
            }
        }
        heads.insert(path, value);
    }
    let CheckoutHead {
        commit,
        digest_input,
    } = head;
    let digest = tokio::task::spawn_blocking(move || {
        let mut manifest = std::collections::BTreeMap::new();
        let mut raw_manifest = std::collections::BTreeMap::new();
        let (d, f) = tree_collect_in(
            &root,
            &paths,
            &index,
            Some(&digest_input),
            &heads,
            (FILE_LIMIT, TOTAL_LIMIT, TIME_LIMIT),
            (with_manifest != ManifestMode::None).then_some(&mut manifest),
            (with_manifest == ManifestMode::Review).then_some(&mut raw_manifest),
            pruned.as_ref(),
        )?;
        let i = ignored.and_then(|l| ignored_tree(&root, &l));
        Ok::<_, CoreError>((d, i, f, manifest, raw_manifest))
    })
    .await
    .map_err(|_| refused())??;
    let manifest = digest.3;
    let raw_manifest = digest.4;
    flags.merge(digest.2);
    // Ignored files are not deliverable content: an ignored digest that could not be
    // observed leaves it `None` (no review waiver can rest on it) but never makes the
    // deliverable observation, or its per-file manifest, incomplete.
    let mut ignored = digest.1;
    if !nested_ignored.is_empty() {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        feed(
            &mut h,
            ignored.as_deref().unwrap_or("unobserved").as_bytes(),
        );
        for (path, nested) in &nested_ignored {
            feed(&mut h, path.as_bytes());
            feed(&mut h, nested.as_deref().unwrap_or("unobserved").as_bytes());
            if nested.is_none() {
                ignored = None;
            }
        }
        if ignored.is_some() {
            ignored = Some(hex(h));
        }
    }
    Ok(Observed {
        digest: digest.0,
        head: commit,
        ignored,
        flags,
        manifest,
        raw_manifest,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_lists_content_hashes_nested_checkouts_and_empty_directories() {
        let base = std::env::temp_dir().join(format!("native-manifest-{}", std::process::id()));
        std::fs::create_dir_all(base.join("empty")).unwrap();
        std::fs::write(base.join("a"), "one").unwrap();
        std::fs::write(base.join("b"), "two").unwrap();
        let heads = [("sub".to_owned(), "abc:nested".to_owned())].into();
        let manifest = |paths: &[u8]| {
            let mut m = std::collections::BTreeMap::new();
            tree_collect(
                &base,
                paths,
                b"",
                None,
                &heads,
                (u64::MAX, u64::MAX, Duration::from_secs(60)),
                Some(&mut m),
            )
            .unwrap();
            m
        };
        let before = manifest(b"a\0b\0");
        assert_eq!(before.len(), 3, "{before:?}");
        assert!(before.contains_key("empty/"));
        std::fs::write(base.join("a"), "changed").unwrap();
        let after = manifest(b"a\0b\0");
        assert_ne!(before["a"], after["a"]);
        assert_eq!(before["b"], after["b"]);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn review_manifest_records_raw_sha256_for_fully_read_files() {
        let base = std::env::temp_dir().join(format!("native-raw-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("one"), b"one").unwrap();
        let heads = std::collections::BTreeMap::new();
        let mut identity = std::collections::BTreeMap::new();
        let mut raw = std::collections::BTreeMap::new();
        tree_collect_in(
            &base,
            b"one\0",
            b"",
            None,
            &heads,
            (u64::MAX, u64::MAX, Duration::from_secs(60)),
            Some(&mut identity),
            Some(&mut raw),
            None,
        )
        .unwrap();
        assert_eq!(
            raw.get("one").map(String::as_str),
            Some("7692c3ad3540bb803c020b3aee66cd8887123234ea0c6e7143c0add73ff431ed")
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "native-effects-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&p).unwrap();
            std::fs::write(p.join("file"), "confirmed").unwrap();
            std::fs::create_dir(p.join(".cortex-dev")).unwrap();
            std::fs::write(p.join(".cortex-dev/private"), "fixture-private").unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    async fn sandbox_available(root: &Path) -> bool {
        match verify(root.to_owned(), &["/bin/true".into()]).await {
            Err(e) => {
                assert_eq!(e.code(), "native_verification_sandbox_unavailable");
                false
            }
            Ok(r) if r["observation"]["failure"] == "native_verification_sandbox_unavailable" => {
                assert_eq!(r["observation"]["complete"], false);
                assert!(r["observation"]["exit_code"].is_null());
                false
            }
            Ok(r) => {
                assert_eq!(r["observation"]["complete"], true);
                assert_eq!(r["observation"]["exit_code"], 0);
                true
            }
        }
    }
    fn git_cmd(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=F",
                "-c",
                "user.email=f@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "protocol.file.allow=always",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
    #[tokio::test]
    async fn symlinks_submodules_large_and_ignored_files_are_observable_not_fatal() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let f = Fixture::new();
        let base = f.0.join("outer");
        let sub_source = f.0.join("sub-source");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&sub_source).unwrap();
        git_cmd(&sub_source, &["init", "-q"]);
        std::fs::write(sub_source.join("lib"), "lib").unwrap();
        git_cmd(&sub_source, &["add", "lib"]);
        git_cmd(&sub_source, &["commit", "-qm", "sub"]);
        git_cmd(&base, &["init", "-q"]);
        std::fs::write(base.join(".gitignore"), "ignored.log\n").unwrap();
        std::fs::write(base.join("real"), "real").unwrap();
        std::fs::create_dir(base.join("dir")).unwrap();
        std::fs::write(base.join("dir/inner"), "inner").unwrap();
        symlink("real", base.join("link-file")).unwrap();
        symlink("dir", base.join("link-dir")).unwrap();
        symlink("/nonexistent/target", base.join("dangling")).unwrap();
        std::fs::write(base.join("big"), vec![7u8; 17 * 1024 * 1024]).unwrap();
        std::fs::write(base.join("ignored.log"), "one").unwrap();
        git_cmd(&base, &["add", "."]);
        git_cmd(
            &base,
            &[
                "submodule",
                "add",
                "-q",
                sub_source.to_str().unwrap(),
                "sub",
            ],
        );
        git_cmd(&base, &["commit", "-qm", "base"]);
        let observe_both = || async {
            let e = observe(base.clone()).await.unwrap();
            (e.project_digest.unwrap(), e.ignored_digest.unwrap())
        };
        let (d0, i0) = observe_both().await;
        assert_eq!(observe_both().await, (d0.clone(), i0.clone()));
        // A retargeted link changes the digest; its target is never followed.
        symlink("elsewhere", base.join("untracked-link")).unwrap();
        let (d1, _) = observe_both().await;
        assert_ne!(d1, d0);
        std::fs::remove_file(base.join("untracked-link")).unwrap();
        std::fs::remove_file(base.join("link-file")).unwrap();
        symlink("dir/inner", base.join("link-file")).unwrap();
        let (d2, _) = observe_both().await;
        assert_ne!(d2, d0);
        // Content behind a symlinked directory is not part of the project listing and is not followed.
        std::fs::write(base.join("dir/inner"), "changed").unwrap();
        assert_ne!(observe_both().await.0, d2);
        std::fs::write(base.join("dir/inner"), "inner").unwrap();
        // A submodule checkout moving to a new commit is observed.
        let before = observe_both().await.0;
        std::fs::write(base.join("sub/new"), "x").unwrap();
        git_cmd(&base.join("sub"), &["add", "new"]);
        git_cmd(&base.join("sub"), &["commit", "-qm", "advance"]);
        assert_ne!(observe_both().await.0, before);
        // A large file is recorded by size/mtime with a marker: growth is seen.
        let before = observe_both().await.0;
        std::fs::write(base.join("big"), vec![7u8; 18 * 1024 * 1024]).unwrap();
        assert_ne!(observe_both().await.0, before);
        // Mode changes and ignored-file changes are seen (ignored ones in their own digest).
        let (before, ignored_before) = observe_both().await;
        std::fs::set_permissions(base.join("real"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        assert_ne!(observe_both().await.0, before);
        std::fs::write(base.join("ignored.log"), "two!").unwrap();
        let (after, ignored_after) = observe_both().await;
        assert_ne!(ignored_after, ignored_before);
        assert_eq!(after, observe_both().await.0);
        // Unreadable content stays an explicit refusal.
        std::fs::set_permissions(base.join("real"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        if unsafe { libc::geteuid() } != 0 {
            let error = observe(base.clone()).await.err().unwrap();
            assert_eq!(error.code(), "project_unobservable_unreadable_content");
        }
        std::fs::set_permissions(base.join("real"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git_cmd(dir, &["init", "-q"]);
        std::fs::write(dir.join("lib"), "lib").unwrap();
        git_cmd(dir, &["add", "lib"]);
        git_cmd(dir, &["commit", "-qm", "init"]);
    }
    #[tokio::test]
    async fn snapshot_reports_the_checkout_head_but_not_an_unborn_head() {
        let f = Fixture::new();
        let base = f.0.join("project");
        std::fs::create_dir(&base).unwrap();
        assert!(observe(base.clone()).await.unwrap().head.is_none());
        git_cmd(&base, &["init", "-q"]);
        assert!(observe(base.clone()).await.unwrap().head.is_none());
        git_cmd(&base, &["commit", "--allow-empty", "-qm", "initial"]);
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&base)
            .output()
            .unwrap();
        assert!(output.status.success());
        let expected = std::str::from_utf8(&output.stdout).unwrap().trim_end();
        assert_eq!(
            observe(base.clone()).await.unwrap().head.as_deref(),
            Some(expected)
        );
        git_cmd(&base, &["checkout", "--quiet", "--detach", expected]);
        assert_eq!(observe(base).await.unwrap().head.as_deref(), Some(expected));
    }

    /// A project whose ignored files list to far more than the general 1 MiB command
    /// output bound is observed completely: a usable manifest and an ignored digest
    /// that moves with ignored changes while the project digest stays.
    #[tokio::test]
    async fn a_large_ignored_listing_is_observed_completely() {
        let f = Fixture::new();
        let base = f.0.join("project");
        init_repo(&base);
        std::fs::write(base.join(".gitignore"), "out/\n").unwrap();
        git_cmd(&base, &["add", ".gitignore"]);
        git_cmd(&base, &["commit", "-qm", "ignore"]);
        let long = "n".repeat(100);
        for d in 0..12 {
            let dir = base.join(format!("out/d{d}"));
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..1000 {
                std::fs::write(dir.join(format!("{long}{i}")), "x").unwrap();
            }
        }
        let listing = git_listing(&base, true).await.unwrap();
        assert!(
            listing.ignored.as_ref().unwrap().len() > OUTPUT,
            "the fixture must exceed the general output bound"
        );
        let (snapshot, manifest) = observe_manifest(base.clone()).await.unwrap();
        assert!(!snapshot.incomplete && !snapshot.metadata_only && !snapshot.cap_reached);
        let i0 = snapshot.ignored_digest.clone().expect("ignored digest");
        let d0 = snapshot.project_digest.clone().unwrap();
        let manifest = manifest.expect("a complete observation has a manifest");
        assert!(manifest.keys().all(|k| !k.starts_with("out/")));
        std::fs::write(base.join(format!("out/d3/{long}7")), "changed").unwrap();
        let (after, again) = observe_manifest(base.clone()).await.unwrap();
        assert!(!after.incomplete && again.is_some());
        assert_eq!(after.project_digest.unwrap(), d0);
        assert_ne!(after.ignored_digest.unwrap(), i0);

        // Set the independent ignored listing limit below the fixture size to
        // exercise actual output truncation without requiring a 256 MiB fixture.
        let listing = git_listing_with_ignored_limit(&base, Some(OUTPUT))
            .await
            .unwrap();
        assert!(listing.ignored.is_none());
        let observed = observe_listed(base, 0, ManifestMode::Review, true, listing)
            .await
            .unwrap();
        assert!(observed.ignored.is_none());
        assert!(!observed.flags.incomplete);
        assert!(!observed.flags.cap_reached);
        assert!(!observed.flags.metadata_only);
        assert_eq!(observed.digest, d0);
        assert_eq!(observed.manifest, manifest);
        assert_eq!(
            observed.raw_manifest.keys().collect::<Vec<_>>(),
            [".gitignore", "lib"]
        );
    }

    #[tokio::test]
    async fn deliverable_content_over_the_file_limit_has_no_usable_manifest() {
        let f = Fixture::new();
        let base = f.0.join("project");
        init_repo(&base);
        // Sparse, so this exercises the real production cap without allocating
        // or hashing GiB of fixture data.
        File::create(base.join("large"))
            .unwrap()
            .set_len(FILE_LIMIT + 1)
            .unwrap();
        let (snapshot, manifest) = observe_manifest(base).await.unwrap();
        assert!(snapshot.cap_reached && snapshot.incomplete && snapshot.metadata_only);
        assert!(manifest.is_none());
    }

    /// The ignored digest ignores listing order and private stores, but records a
    /// listed entry that vanished before the metadata pass.
    #[test]
    fn ignored_digest_is_order_free_and_survives_vanished_entries() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.0.join("out")).unwrap();
        std::fs::write(f.0.join("out/a"), "a").unwrap();
        std::fs::write(f.0.join("out/b"), "b").unwrap();
        let sorted = ignored_tree(&f.0, b"out/a\0out/b\0").unwrap();
        assert_eq!(
            ignored_tree(&f.0, b"out/b\0out/a\0out/b\0").unwrap(),
            sorted
        );
        assert_eq!(
            ignored_tree(&f.0, b"out/a\0out/b\0sub/.cortex/x\0").unwrap(),
            sorted
        );
        let vanished = ignored_tree(&f.0, b"out/a\0out/b\0out/gone\0").unwrap();
        assert_ne!(vanished, sorted);
        assert!(ignored_tree(&f.0, b"../escape\0").is_none());
        assert!(ignored_tree_within(&f.0, b"out/a\0", Duration::ZERO).is_none());
    }

    #[test]
    fn ignored_digest_is_unobserved_across_changes_to_an_unreadable_entry() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let closed = f.0.join("out/closed");
        std::fs::create_dir_all(&closed).unwrap();
        let entry = File::create(closed.join("c")).unwrap();
        let listing = b"out/closed/c\0";
        let readable = ignored_tree(&f.0, listing).unwrap();

        // Keep a descriptor so the entry can change without restoring traversal.
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let lookup_failed = lookup(&File::open(&f.0).unwrap(), "out/closed/c", false).is_err();
        let before = ignored_tree(&f.0, listing);
        entry.set_len(42).unwrap();
        let after = ignored_tree(&f.0, listing);
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();

        // A privileged test process may bypass the mode bits. The next test
        // exercises the error branch independently of permissions.
        if lookup_failed {
            assert!(before.is_none());
            assert!(after.is_none());
        }
        assert_ne!(ignored_tree(&f.0, listing).unwrap(), readable);
    }

    #[test]
    fn repeated_metadata_lookup_failure_never_yields_an_ignored_digest() {
        let f = Fixture::new();
        // Simulate a listing that cannot be resolved by a single metadata lookup.
        // Unlike chmod, this still triggers a lookup error when tests run as root.
        let name = "x".repeat(libc::PATH_MAX as usize);
        assert!(lookup(&File::open(&f.0).unwrap(), &name, false).is_err());
        assert!(ignored_tree(&f.0, name.as_bytes()).is_none());
        let unsorted = format!("z\0{name}\0");
        assert!(ignored_tree(&f.0, unsorted.as_bytes()).is_none());
    }

    #[tokio::test]
    async fn empty_directories_inside_ignored_trees_are_not_walked() {
        let f = Fixture::new();
        let base = f.0.join("project");
        init_repo(&base);
        std::fs::write(base.join(".gitignore"), "target/\n").unwrap();
        git_cmd(&base, &["add", ".gitignore"]);
        git_cmd(&base, &["commit", "-qm", "ignore"]);
        std::fs::create_dir_all(base.join("target/debug/deps")).unwrap();
        let full = observe(base.clone()).await.unwrap();
        let d0 = full.project_digest.clone().unwrap();
        assert!(!full.incomplete);
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d0);
        // Build output under an ignored tree is not project content.
        std::fs::create_dir_all(base.join("target/debug/incremental/x")).unwrap();
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d0);
        assert_eq!(
            observe(base.clone()).await.unwrap().project_digest.unwrap(),
            d0
        );
        // An empty directory elsewhere still is.
        std::fs::create_dir_all(base.join("docs/empty")).unwrap();
        let d1 = observe_digest(base.clone()).await.unwrap();
        assert_ne!(d1, d0);
        assert_eq!(
            observe(base.clone()).await.unwrap().project_digest.unwrap(),
            d1
        );
    }
    /// A plain project folder (in no Git repository) is observed completely by a walk:
    /// stable, blind to the private stores, moved by every write, delete, mode change and
    /// empty directory, with a usable per-file manifest and an (empty) ignored digest.
    #[tokio::test]
    async fn a_plain_folder_without_git_is_observed_by_a_walk() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let base = f.0.join("plain");
        std::fs::create_dir(&base).unwrap();
        assert!(
            no_repository(&base).await,
            "the fixture must be in no repository"
        );
        // An empty folder is observable and complete.
        let empty = observe(base.clone()).await.unwrap();
        assert!(!empty.incomplete && !empty.metadata_only && !empty.cap_reached);
        assert!(empty.ignored_digest.is_some());
        let d_empty = empty.project_digest.unwrap();
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d_empty);
        std::fs::create_dir_all(base.join("docs/notes")).unwrap();
        std::fs::write(base.join("docs/notes/a.md"), "one").unwrap();
        std::fs::write(base.join("top.txt"), "top").unwrap();
        std::os::unix::fs::symlink("top.txt", base.join("link")).unwrap();
        let (snapshot, manifest) = observe_manifest(base.clone()).await.unwrap();
        assert!(!snapshot.incomplete && !snapshot.metadata_only && !snapshot.cap_reached);
        let d0 = snapshot.project_digest.clone().unwrap();
        assert_ne!(d0, d_empty);
        let manifest = manifest.expect("a complete plain observation has a manifest");
        assert_eq!(
            manifest.keys().collect::<Vec<_>>(),
            ["docs/notes/a.md", "link", "top.txt"]
        );
        // Deterministic, and the digest-only pass agrees with the full one.
        assert_eq!(
            observe(base.clone()).await.unwrap().project_digest.unwrap(),
            d0
        );
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d0);
        // The private stores and dev markers are never project content.
        std::fs::create_dir_all(base.join(".cortex/t")).unwrap();
        std::fs::write(base.join(".cortex/t/pipeline.md"), "private").unwrap();
        std::fs::create_dir_all(base.join(".cortex-dev")).unwrap();
        std::fs::write(base.join(".cortex-dev/cortex.sqlite3"), "private").unwrap();
        std::fs::write(base.join(".cortex-dev-owner.json"), "{}").unwrap();
        std::fs::write(base.join(".cortex-workspace-dev.json"), "{}").unwrap();
        std::fs::create_dir_all(base.join("docs/.cortex")).unwrap();
        std::fs::write(base.join("docs/.cortex/x"), "nested store").unwrap();
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d0);
        // A write, a new file, a mode change and a delete each move the digest; the
        // manifest attributes the change to exactly the file.
        std::fs::write(base.join("docs/notes/a.md"), "two").unwrap();
        let (after, changed) = observe_manifest(base.clone()).await.unwrap();
        let changed = changed.unwrap();
        assert_ne!(after.project_digest.clone().unwrap(), d0);
        assert_ne!(changed["docs/notes/a.md"], manifest["docs/notes/a.md"]);
        assert_eq!(changed["top.txt"], manifest["top.txt"]);
        std::fs::write(base.join("docs/notes/a.md"), "one").unwrap();
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d0);
        std::fs::write(base.join("new.txt"), "new").unwrap();
        let (_, added) = observe_manifest(base.clone()).await.unwrap();
        assert!(added.unwrap().contains_key("new.txt"));
        std::fs::remove_file(base.join("new.txt")).unwrap();
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d0);
        std::fs::set_permissions(base.join("top.txt"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        assert_ne!(observe_digest(base.clone()).await.unwrap(), d0);
        std::fs::remove_file(base.join("top.txt")).unwrap();
        let (_, deleted) = observe_manifest(base.clone()).await.unwrap();
        assert!(!deleted.unwrap().contains_key("top.txt"));
        std::fs::write(base.join("top.txt"), "top").unwrap();
        std::fs::set_permissions(base.join("top.txt"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let d1 = observe_digest(base.clone()).await.unwrap();
        // An empty directory is a project change, as in a repository.
        std::fs::create_dir(base.join("hollow")).unwrap();
        assert_ne!(observe_digest(base.clone()).await.unwrap(), d1);
        std::fs::remove_dir(base.join("hollow")).unwrap();
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d1);
        // A nested repository inside the plain folder is observed by its own pass.
        init_repo(&base.join("nested"));
        let with_nested = observe(base.clone()).await.unwrap();
        assert!(!with_nested.incomplete);
        let d2 = with_nested.project_digest.unwrap();
        assert_ne!(d2, d1);
        std::fs::write(base.join("nested/lib"), "edited").unwrap();
        assert_ne!(observe_digest(base.clone()).await.unwrap(), d2);
        std::fs::write(base.join("nested/lib"), "lib").unwrap();
        assert_eq!(observe_digest(base.clone()).await.unwrap(), d2);
        // An unreadable directory is a refusal, never a partial listing.
        if unsafe { libc::geteuid() } != 0 {
            std::fs::create_dir(base.join("closed")).unwrap();
            std::fs::set_permissions(base.join("closed"), std::fs::Permissions::from_mode(0o000))
                .unwrap();
            let error = observe(base.clone()).await.err().unwrap();
            assert_eq!(error.code(), "project_unobservable_unreadable_content");
            std::fs::set_permissions(base.join("closed"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
    }
    /// "No repository" is only Git's own answer: a broken repository (a `.git` entry Git
    /// cannot use) stays a refusal and is never observed as a plain folder.
    #[tokio::test]
    async fn a_broken_repository_is_refused_not_walked() {
        let f = Fixture::new();
        let base = f.0.join("broken");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(base.join("file"), "x").unwrap();
        std::fs::write(base.join(".git"), "gitdir: /nonexistent/cortex-fixture\n").unwrap();
        assert!(!no_repository(&base).await);
        assert_eq!(
            observe(base.clone()).await.err().unwrap().code(),
            "project_revision_unavailable"
        );
        assert_eq!(
            observe_digest(base).await.err().unwrap().code(),
            "project_revision_unavailable"
        );
    }
    /// A repository is observed by Git exactly as before: its digest differs from the
    /// same files in a plain folder, which never stand in for a repository's.
    #[tokio::test]
    async fn a_repository_and_the_same_plain_files_have_different_digests() {
        let f = Fixture::new();
        let repo = f.0.join("repo");
        let plain = f.0.join("plain");
        init_repo(&repo);
        std::fs::create_dir(&plain).unwrap();
        std::fs::write(plain.join("lib"), "lib").unwrap();
        assert!(!no_repository(&repo).await);
        assert_ne!(
            observe_digest(repo).await.unwrap(),
            observe_digest(plain).await.unwrap()
        );
    }
    #[tokio::test]
    async fn nested_dirty_state_empty_directories_and_metadata_only_are_observed() {
        let f = Fixture::new();
        let base = f.0.join("outer");
        let sub_source = f.0.join("sub-source");
        init_repo(&sub_source);
        init_repo(&base);
        git_cmd(
            &base,
            &[
                "submodule",
                "add",
                "-q",
                sub_source.to_str().unwrap(),
                "sub",
            ],
        );
        git_cmd(&base, &["commit", "-qm", "sub"]);
        // An untracked nested repository.
        init_repo(&base.join("nested"));
        let snap = || async { observe(base.clone()).await.unwrap() };
        let clean = snap().await;
        assert!(!clean.metadata_only && !clean.incomplete && !clean.cap_reached);
        let d0 = clean.project_digest.clone().unwrap();
        // Dirty tracked file inside the submodule (HEAD unchanged), then a second
        // different edit that leaves the same porcelain status.
        std::fs::write(base.join("sub/lib"), "dirty one").unwrap();
        let d1 = snap().await.project_digest.unwrap();
        assert_ne!(d1, d0);
        std::fs::write(base.join("sub/lib"), "dirty two").unwrap();
        let d2 = snap().await.project_digest.unwrap();
        assert_ne!(d2, d1);
        std::fs::write(base.join("sub/lib"), "lib").unwrap();
        assert_eq!(snap().await.project_digest.unwrap(), d0);
        // Untracked file and tracked edit in an untracked nested repository.
        std::fs::write(base.join("nested/extra"), "x").unwrap();
        let d3 = snap().await.project_digest.unwrap();
        assert_ne!(d3, d0);
        std::fs::remove_file(base.join("nested/extra")).unwrap();
        std::fs::write(base.join("nested/lib"), "edited").unwrap();
        assert_ne!(snap().await.project_digest.unwrap(), d0);
        std::fs::write(base.join("nested/lib"), "lib").unwrap();
        assert_eq!(snap().await.project_digest.unwrap(), d0);
        // Empty directory creation and removal, also inside a nested checkout.
        for dir in ["empty", "sub/hollow", "nested/hollow"] {
            std::fs::create_dir(base.join(dir)).unwrap();
            assert_ne!(snap().await.project_digest.unwrap(), d0, "{dir}");
            std::fs::remove_dir(base.join(dir)).unwrap();
            assert_eq!(snap().await.project_digest.unwrap(), d0, "{dir}");
        }
        // Ignored files are not deliverable: they never mark the observation metadata-only
        // or incomplete and never change the project digest; only the ignored digest moves.
        std::fs::write(base.join(".gitignore"), "ignored.log\ntarget/\n").unwrap();
        git_cmd(&base, &["add", ".gitignore"]);
        git_cmd(&base, &["commit", "-qm", "ignore"]);
        let d_ign = snap().await;
        let (pd, id0) = (
            d_ign.project_digest.clone().unwrap(),
            d_ign.ignored_digest.clone().unwrap(),
        );
        std::fs::create_dir(base.join("target")).unwrap();
        std::fs::write(base.join("target/out"), "build").unwrap();
        std::fs::write(base.join("ignored.log"), "one").unwrap();
        let with = snap().await;
        assert!(!with.metadata_only && !with.incomplete);
        assert_eq!(with.project_digest.clone().unwrap(), pd);
        let id1 = with.ignored_digest.clone().unwrap();
        assert_ne!(id1, id0);
        assert_eq!(
            snap().await.ignored_digest.unwrap(),
            id1,
            "unchanged ignored files are stable"
        );
        std::fs::write(base.join("target/out"), "changed!").unwrap();
        assert_ne!(snap().await.ignored_digest.unwrap(), id1);
        // An ignored file inside a nested checkout moves the ignored digest, not the project's.
        std::fs::remove_dir_all(base.join("target")).unwrap();
        std::fs::remove_file(base.join("ignored.log")).unwrap();
        assert_eq!(snap().await.ignored_digest.unwrap(), id0);
        std::fs::write(base.join("nested/.gitignore"), "cache\n").unwrap();
        git_cmd(&base.join("nested"), &["add", ".gitignore"]);
        git_cmd(&base.join("nested"), &["commit", "-qm", "ignore"]);
        let nd = snap().await;
        let (npd, nid) = (nd.project_digest.unwrap(), nd.ignored_digest.unwrap());
        std::fs::write(base.join("nested/cache"), "x").unwrap();
        let nd2 = snap().await;
        assert_eq!(nd2.project_digest.unwrap(), npd);
        assert_ne!(nd2.ignored_digest.unwrap(), nid);
        std::fs::remove_file(base.join("nested/cache")).unwrap();
        // Large tracked content is hashed in full: a same-size edit with the mtime
        // restored is still seen, and nothing is metadata-only.
        std::fs::write(base.join("big"), vec![1u8; 17 * 1024 * 1024]).unwrap();
        let before = snap().await;
        assert!(!before.metadata_only && !before.cap_reached && !before.incomplete);
        let mtime = std::fs::metadata(base.join("big"))
            .unwrap()
            .modified()
            .unwrap();
        let mut edited = vec![1u8; 17 * 1024 * 1024];
        edited[9 * 1024 * 1024] = 2;
        std::fs::write(base.join("big"), &edited).unwrap();
        std::fs::File::options()
            .write(true)
            .open(base.join("big"))
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_ne!(snap().await.project_digest, before.project_digest);
        // Past a safety bound a deliverable file is metadata-only and the observation incomplete.
        let (_, flags) = tree_within(
            &base,
            b"big\0",
            b"",
            None,
            &Default::default(),
            (1024, u64::MAX, Duration::from_secs(60)),
        )
        .unwrap();
        assert!(flags.metadata_only && flags.cap_reached && flags.incomplete);
        let (_, flags) = tree_within(
            &base,
            b"big\0",
            b"",
            None,
            &Default::default(),
            (u64::MAX, 1024, Duration::from_secs(60)),
        )
        .unwrap();
        assert!(flags.incomplete && flags.cap_reached);
        let (_, flags) = tree_within(
            &base,
            b"big\0",
            b"",
            None,
            &Default::default(),
            (u64::MAX, u64::MAX, Duration::ZERO),
        )
        .unwrap();
        assert!(flags.incomplete);
        // A special file (fifo) cannot be content-hashed: incomplete.
        let fifo = std::ffi::CString::new(base.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let (_, flags) = tree_within(
            &base,
            b"pipe\0",
            b"",
            None,
            &Default::default(),
            (u64::MAX, u64::MAX, Duration::from_secs(60)),
        )
        .unwrap();
        assert!(flags.incomplete && flags.metadata_only);
        std::fs::remove_file(base.join("pipe")).unwrap();
        std::fs::remove_file(base.join("big")).unwrap();
        // A nested checkout deeper than the observation depth is incomplete, not clean.
        let mut deep = base.join("nested");
        for i in 0..6 {
            deep = deep.join(format!("n{i}"));
            init_repo(&deep);
        }
        assert!(snap().await.incomplete);
    }
    #[tokio::test]
    async fn readonly_verifier_reads_project_but_cannot_mutate_or_read_private_store() {
        let f = Fixture::new();
        if !sandbox_available(&f.0).await {
            // The actual verifier refused before running any proposed command.
            assert_eq!(
                std::fs::read_to_string(f.0.join("file")).unwrap(),
                "confirmed"
            );
            return;
        }
        let run = |args: Vec<String>| {
            let root = f.0.clone();
            async move { verify(root, &args).await }
        };
        let r = run(vec!["/bin/cat".into(), "file".into()]).await.unwrap();
        assert_eq!(r["observation"]["exit_code"], 0);
        assert_eq!(r["observation"]["stdout"], "confirmed");
        let r = run(vec!["/usr/bin/touch".into(), "created".into()])
            .await
            .unwrap();
        // An overlay accepts the write; the fallback refuses it. Neither
        // may change the reviewed project on disk.
        assert_eq!(r["observation"]["complete"], true);
        assert!(r["observation"]["exit_code"].is_number());
        assert!(!f.0.join("created").exists());
        let r = run(vec!["/bin/cat".into(), ".cortex-dev/private".into()])
            .await
            .unwrap();
        assert_ne!(r["observation"]["exit_code"], 0);
        assert_eq!(r["observation"]["stdout"], "");
        let r = run(vec![
            "/usr/bin/truncate".into(),
            "-s".into(),
            "0".into(),
            "file".into(),
        ])
        .await
        .unwrap();
        assert_eq!(r["observation"]["complete"], true);
        assert!(r["observation"]["exit_code"].is_number());
        assert_eq!(
            std::fs::read_to_string(f.0.join("file")).unwrap(),
            "confirmed"
        );
    }
    #[tokio::test]
    async fn sandboxed_verifier_retains_only_bounded_output() {
        let f = Fixture::new();
        if !sandbox_available(&f.0).await {
            return;
        }
        let r = verify(
            f.0.clone(),
            &[
                "/usr/bin/head".into(),
                "-c".into(),
                "1048577".into(),
                "/dev/zero".into(),
            ],
        )
        .await
        .unwrap();
        // Device reads remain usable; full output limits still block a passing check.
        assert_eq!(r["observation"]["output_truncated"], true);
        assert_eq!(r["observation"]["complete"], false);
        assert_eq!(r["observation"]["exit_code"], 0);
        assert_eq!(r["observation"]["failure"], "command_output_limit");
    }
    #[tokio::test]
    async fn capture_distinguishes_timeout_signal_refusal_and_output_limit() {
        // Capture itself does not need a user namespace. Keep this test active
        // even where the real sandbox must fail closed before executing argv.
        let f = Fixture::new();
        let mut c = command(
            &f.0,
            &[
                "/usr/bin/head".into(),
                "-c".into(),
                "1048577".into(),
                "/dev/zero".into(),
            ],
        );
        c.env_clear();
        let (_cancel, receiver) = tokio::sync::watch::channel(false);
        let r = capture(c, Duration::from_secs(2), receiver).await.unwrap();
        assert!(r.truncated);
        assert!(!r.complete);
        assert_eq!(r.code, Some(0));
        assert_eq!(r.failure, Some("command_output_limit"));
        assert_eq!(r.stdout.len(), OUTPUT);
        let c = command(&f.0, &["/bin/sleep".into(), "2".into()]);
        let (_cancel, receiver) = tokio::sync::watch::channel(false);
        let r = capture(c, Duration::from_millis(20), receiver)
            .await
            .unwrap();
        assert!(!r.complete);
        assert_eq!(r.code, None);
        assert_eq!(r.failure, Some("command_timeout"));
        assert!(!r.truncated);

        let (_cancel, receiver) = tokio::sync::watch::channel(false);
        let c = command(
            &f.0,
            &["/bin/sh".into(), "-c".into(), "kill -TERM $$".into()],
        );
        let r = capture(c, Duration::from_secs(2), receiver).await.unwrap();
        assert!(!r.complete);
        assert_eq!(r.code, None);
        assert_eq!(r.failure, Some("command_signalled"));

        // A program's own nonzero exit (including a refusal) is terminal.
        let (_cancel, receiver) = tokio::sync::watch::channel(false);
        let c = command(&f.0, &["/bin/sh".into(), "-c".into(), "exit 2".into()]);
        let r = capture(c, Duration::from_secs(2), receiver).await.unwrap();
        assert!(r.complete);
        assert_eq!(r.code, Some(2));
        assert_eq!(r.failure, None);

        let (_cancel, receiver) = tokio::sync::watch::channel(true);
        let c = command(&f.0, &["/bin/sleep".into(), "2".into()]);
        let r = capture(c, Duration::from_secs(2), receiver).await.unwrap();
        assert!(!r.complete);
        assert_eq!(r.code, None);
        assert_eq!(r.failure, Some("owner_stopped"));
    }
    /// A program the owner installed through a version manager (a `/usr/local/bin`-style
    /// link into a directory under the read-only home) is found by the coordinator's plan
    /// preflight exactly where the final verifier launch finds it, while a missing program
    /// is still refused.
    #[tokio::test]
    async fn preflight_resolves_bare_programs_on_the_verifier_search_path() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let f = Fixture::new();
        if !sandbox_available(&f.0).await {
            return;
        }
        let home = f.0.join("home");
        let installed = home.join(".nvm/versions/tool/v1/bin");
        std::fs::create_dir_all(&installed).unwrap();
        let program = installed.join("cortex-fixture-tool");
        std::fs::write(&program, "#!/bin/sh\necho fixture-tool-v1\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let links = f.0.join("local-bin");
        std::fs::create_dir(&links).unwrap();
        symlink(&program, links.join("cortex-fixture-tool")).unwrap();
        let project = f.0.join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("file"), "x").unwrap();
        let environment: BTreeMap<OsString, OsString> = [
            ("HOME".into(), home.clone().into_os_string()),
            (
                "PATH".into(),
                format!("{}:/usr/bin:/bin", links.display()).into(),
            ),
        ]
        .into();
        let (_cancel, receiver) = tokio::sync::watch::channel(false);
        let launched = verify_in(
            project.clone(),
            &["cortex-fixture-tool".into()],
            receiver,
            Some(environment.clone()),
        )
        .await
        .unwrap();
        assert_eq!(launched["observation"]["exit_code"], 0, "{launched}");
        assert_eq!(launched["observation"]["stdout"], "fixture-tool-v1\n");
        verification_preflight_in(
            project.clone(),
            &json!([["cortex-fixture-tool", "--version"]]),
            environment.clone(),
        )
        .await
        .expect("an installed program on the command search path is available");
        let missing = verification_preflight_in(
            project.clone(),
            &json!([["cortex-fixture-missing-tool"]]),
            environment.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(missing.code(), "verification_executable_unavailable");
        // A link whose target is gone is not an installed program.
        std::fs::remove_file(&program).unwrap();
        let dangling =
            verification_preflight_in(project, &json!([["cortex-fixture-tool"]]), environment)
                .await
                .unwrap_err();
        assert_eq!(dangling.code(), "verification_executable_unavailable");
    }
    /// Plan admission and the program probe (`check_preflight` calls the same
    /// `verification_preflight`) refused a program the owner's terminal has, because
    /// the service's session-manager PATH lacks the shell's directories. With the
    /// path read from the owner's login shell, admission accepts it and the final
    /// verifier runs exactly that program.
    #[tokio::test]
    async fn admission_and_the_verifier_use_the_path_read_from_the_owners_shell() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        if !sandbox_available(&f.0).await {
            eprintln!("skipped: bubblewrap cannot create user namespaces on this host");
            return;
        }
        let s = crate::tool_path::tests::Scratch::new("admission");
        let bin = s.dir("home/.nvm/versions/node/v24.1.0/bin");
        let program = bin.join("cortex-fixture-npm");
        std::fs::write(&program, "#!/bin/sh\necho fixture-npm \"$1\"\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shell = crate::tool_path::tests::login_shell(
            &s,
            &format!(
                ". /dev/null\necho 'Now using node v24'\nexport PATH={}:$PATH\n",
                bin.display()
            ),
        );
        let home = s.0.join("home");
        let systemd_path = "/usr/local/bin:/usr/bin:/bin";
        let detected = crate::tool_path::detect(
            &shell,
            &home,
            std::ffi::OsStr::new(systemd_path),
            &[],
            Duration::from_secs(5),
        );
        let project = s.dir("project");
        std::fs::write(project.join("file"), "x").unwrap();
        let before: BTreeMap<OsString, OsString> = [
            ("HOME".into(), home.clone().into_os_string()),
            ("PATH".into(), systemd_path.into()),
        ]
        .into();
        let check = json!([["cortex-fixture-npm", "--version"]]);
        let refused = verification_preflight_in(project.clone(), &check, before)
            .await
            .unwrap_err();
        assert_eq!(refused.code(), "verification_executable_unavailable");
        let after: BTreeMap<OsString, OsString> = [
            ("HOME".into(), home.into_os_string()),
            (
                "PATH".into(),
                std::env::join_paths(&detected.directories).unwrap(),
            ),
        ]
        .into();
        verification_preflight_in(project.clone(), &check, after.clone())
            .await
            .expect("the program on the owner's shell path is admitted");
        let (_cancel, receiver) = tokio::sync::watch::channel(false);
        let ran = verify_in(
            project,
            &["cortex-fixture-npm".into(), "--version".into()],
            receiver,
            Some(after),
        )
        .await
        .unwrap();
        assert_eq!(ran["observation"]["exit_code"], 0, "{ran}");
        assert_eq!(ran["observation"]["stdout"], "fixture-npm --version\n");
    }
}
/// Safety bounds for hashing deliverable content. Real projects stay far below them;
/// exceeding any one marks the observation incomplete (no waiver), never a pass.
const FILE_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 8 * 1024 * 1024 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(300);
const NAME_LIMIT: usize = 100_000;
fn unreadable() -> CoreError {
    CoreError::Refused("project_unobservable_unreadable_content")
}
fn changed() -> CoreError {
    CoreError::Refused("project_unobservable_changed_while_reading")
}
/// What one project path is, observed without ever following a link.
struct Seen {
    kind: u8,
    mode: u32,
    size: u64,
    mtime: (i64, i64),
    target: Vec<u8>,
    file: Option<File>,
}
const MISSING: u8 = 0;
const REGULAR: u8 = 1;
const LINK: u8 = 2;
const DIRECTORY: u8 = 3;
const SPECIAL: u8 = 5;
const THROUGH_LINK: u8 = 6;
fn plain(kind: u8) -> Seen {
    Seen {
        kind,
        mode: 0,
        size: 0,
        mtime: (0, 0),
        target: vec![],
        file: None,
    }
}
fn openat2(dir: i32, name: &std::ffi::CStr, flags: i32, resolve: u64) -> std::io::Result<File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: flags as u64,
        mode: 0,
        resolve,
    };
    use std::os::fd::FromRawFd;
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir,
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd as i32) })
    }
}
/// Resolve `name` below `root` (beneath, no symlinks in any parent) and describe
/// the final component without following it. A symlink is recorded by its target
/// string; a directory is a gitlink or nested repository; `open` also opens a
/// regular file for reading.
fn lookup(root: &File, name: &str, open: bool) -> CoreResult<Seen> {
    use std::os::fd::FromRawFd;
    let name = name.trim_end_matches('/');
    let (parent, base) = match name.rsplit_once('/') {
        Some((p, b)) => (Some(p), b),
        None => (None, name),
    };
    let held;
    let dir = match parent {
        Some(p) => {
            let c = std::ffi::CString::new(p).map_err(|_| refused())?;
            match openat2(
                root.as_raw_fd(),
                &c,
                libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
                0x08 | 0x04,
            ) {
                Ok(f) => {
                    held = f;
                    held.as_raw_fd()
                }
                Err(e) => {
                    return match e.raw_os_error() {
                        Some(libc::ENOENT) => Ok(plain(MISSING)),
                        Some(libc::ELOOP | libc::ENOTDIR | libc::EXDEV) => Ok(plain(THROUGH_LINK)),
                        _ => Err(unreadable()),
                    }
                }
            }
        }
        None => root.as_raw_fd(),
    };
    let base = std::ffi::CString::new(base).map_err(|_| refused())?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir, base.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ENOENT) => Ok(plain(MISSING)),
            Some(libc::ENOTDIR) => Ok(plain(THROUGH_LINK)),
            _ => Err(unreadable()),
        };
    }
    let mut seen = Seen {
        kind: SPECIAL,
        mode: st.st_mode,
        size: st.st_size as u64,
        mtime: (st.st_mtime, st.st_mtime_nsec),
        target: vec![],
        file: None,
    };
    match st.st_mode & libc::S_IFMT {
        libc::S_IFLNK => {
            let mut buffer = vec![0u8; 8192];
            let n = unsafe {
                libc::readlinkat(dir, base.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len())
            };
            if n < 0 || n as usize >= buffer.len() {
                return Err(unreadable());
            }
            buffer.truncate(n as usize);
            seen.kind = LINK;
            seen.target = buffer;
        }
        libc::S_IFDIR => seen.kind = DIRECTORY,
        libc::S_IFREG => {
            seen.kind = REGULAR;
            if open {
                let fd = unsafe {
                    libc::openat(
                        dir,
                        base.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    return Err(match std::io::Error::last_os_error().raw_os_error() {
                        Some(libc::ENOENT | libc::ELOOP) => changed(),
                        _ => unreadable(),
                    });
                }
                seen.file = Some(unsafe { File::from_raw_fd(fd) });
            }
        }
        _ => {}
    }
    Ok(seen)
}
fn feed(h: &mut sha2::Sha256, bytes: &[u8]) {
    use sha2::Digest;
    h.update((bytes.len() as u64).to_be_bytes());
    h.update(bytes);
}
fn hex(h: sha2::Sha256) -> String {
    use sha2::Digest;
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn checked_name(name: &[u8]) -> CoreResult<&str> {
    let name =
        std::str::from_utf8(name).map_err(|_| CoreError::Refused("project_unobservable_path"))?;
    if name
        .split('/')
        .any(|c| ["..", ".cortex", ".cortex-dev", ".git"].contains(&c))
        || name.starts_with('/')
    {
        return Err(CoreError::Refused("project_unobservable_path"));
    }
    Ok(name)
}
/// Digest of the project: index, HEAD and every listed path (deliverable content,
/// nested checkouts included). Regular files are hashed in full by streaming within
/// generous safety bounds (size, total, time); past a bound a file is recorded by size,
/// mtime and mode with a marker and the observation is incomplete (no waiver); symlinks by their target string;
/// gitlinks by the commit their checkout is at. Genuinely unreadable content is an
/// explicit refusal, never a silent pass.
#[cfg(test)]
fn tree(
    root: &Path,
    paths: &[u8],
    index: &[u8],
    head: Option<&[u8]>,
    heads: &std::collections::BTreeMap<String, String>,
) -> CoreResult<(String, Flags)> {
    tree_within(
        root,
        paths,
        index,
        head,
        heads,
        (FILE_LIMIT, TOTAL_LIMIT, TIME_LIMIT),
    )
}
#[cfg(test)]
fn tree_within(
    root: &Path,
    paths: &[u8],
    index: &[u8],
    head: Option<&[u8]>,
    heads: &std::collections::BTreeMap<String, String>,
    limits: (u64, u64, Duration),
) -> CoreResult<(String, Flags)> {
    tree_collect(root, paths, index, head, heads, limits, None)
}
#[cfg(test)]
fn tree_collect(
    root: &Path,
    paths: &[u8],
    index: &[u8],
    head: Option<&[u8]>,
    heads: &std::collections::BTreeMap<String, String>,
    limits: (u64, u64, Duration),
    manifest: Option<&mut std::collections::BTreeMap<String, String>>,
) -> CoreResult<(String, Flags)> {
    tree_collect_in(
        root, paths, index, head, heads, limits, manifest, None, None,
    )
}
/// `tree_collect` whose empty-directory walk skips the `pruned` ignored directories.
#[allow(clippy::too_many_arguments)]
fn tree_collect_in(
    root: &Path,
    paths: &[u8],
    index: &[u8],
    head: Option<&[u8]>,
    heads: &std::collections::BTreeMap<String, String>,
    (file_limit, total_limit, time_limit): (u64, u64, Duration),
    mut manifest: Option<&mut std::collections::BTreeMap<String, String>>,
    mut raw_manifest: Option<&mut std::collections::BTreeMap<String, String>>,
    pruned: Option<&BTreeSet<Vec<u8>>>,
) -> CoreResult<(String, Flags)> {
    use sha2::Digest;
    let mut flags = Flags::default();
    let mut h = sha2::Sha256::new();
    feed(&mut h, index);
    feed(&mut h, head.unwrap_or(b"unborn"));
    let names = paths
        .split(|c| *c == 0)
        .filter(|p| !p.is_empty())
        .collect::<BTreeSet<_>>();
    if names.len() > NAME_LIMIT {
        return Err(CoreError::Refused("project_unobservable_too_many_paths"));
    }
    // Empty directories are a walk of their own (see below): it runs while the files are hashed.
    std::thread::scope(|scope| {
        let walk = scope.spawn(|| empty_directories(root, pruned));
        let dir = File::open(root)?;
        let started = std::time::Instant::now();
        let mut hashed = 0u64;
        for name in names {
            let name = checked_name(name)?;
            let mut seen = lookup(&dir, name, true)?;
            feed(&mut h, name.as_bytes());
            h.update([seen.kind]);
            // Per-file identity for change attribution (a content hash when streamed).
            let mut entry = sha2::Sha256::new();
            entry.update([seen.kind]);
            entry.update(seen.mode.to_be_bytes());
            match seen.kind {
                REGULAR => {
                    let mut file = seen.file.take().unwrap();
                    let before = file.metadata()?;
                    h.update((seen.mode & 0o7777).to_be_bytes());
                    h.update(before.len().to_be_bytes());
                    if before.len() <= file_limit
                        && hashed + before.len() <= total_limit
                        && started.elapsed() <= time_limit
                    {
                        let mut buffer = vec![0u8; 64 * 1024];
                        let mut raw_entry = raw_manifest.as_ref().map(|_| sha2::Sha256::new());
                        let mut read = 0u64;
                        loop {
                            let n = file.read(&mut buffer).map_err(|_| unreadable())?;
                            if n == 0 {
                                break;
                            }
                            read += n as u64;
                            if read > before.len() {
                                return Err(changed());
                            }
                            h.update(&buffer[..n]);
                            entry.update(&buffer[..n]);
                            if let Some(raw_entry) = raw_entry.as_mut() {
                                raw_entry.update(&buffer[..n]);
                            }
                        }
                        let after = file.metadata()?;
                        if read != before.len() || before.modified()? != after.modified()? {
                            return Err(changed());
                        }
                        hashed += read;
                        h.update(b"content");
                        if let (Some(raw_manifest), Some(raw_entry)) =
                            (raw_manifest.as_deref_mut(), raw_entry)
                        {
                            raw_manifest.insert(name.to_owned(), hex(raw_entry));
                        }
                    } else {
                        // Over the limits: size, mtime and mode stand in for the content.
                        // That is metadata only: restoring the mtime defeats it, so no waiver.
                        flags.metadata_only = true;
                        flags.cap_reached = true;
                        flags.incomplete = true;
                        h.update(b"large-file");
                        h.update(seen.mtime.0.to_be_bytes());
                        h.update(seen.mtime.1.to_be_bytes());
                    }
                }
                LINK => {
                    feed(&mut h, &seen.target);
                    entry.update(&seen.target);
                }
                DIRECTORY => {
                    let nested = heads
                        .get(name.trim_end_matches('/'))
                        .map(String::as_bytes)
                        .unwrap_or(b"no-checkout");
                    feed(&mut h, nested);
                    entry.update(nested);
                }
                SPECIAL => {
                    // A fifo, socket or device has no content to hash: deny the waiver.
                    flags.metadata_only = true;
                    flags.incomplete = true;
                    h.update(seen.mode.to_be_bytes())
                }
                _ => {}
            }
            if let Some(m) = manifest.as_deref_mut() {
                m.insert(name.to_owned(), hex(entry));
            }
        }
        // Git does not track empty directories; creating or removing one is a project change.
        let (empty, complete) = walk.join().map_err(|_| unreadable())?;
        flags.incomplete |= !complete;
        if !complete {
            // An incomplete walk (over `WALK_LIMIT` entries, or an unreadable directory)
            // observed nothing reliable: this digest is unique to this observation, so it
            // never compares as unchanged with another one.
            static UNOBSERVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            h.update(b"empty-directories-unobserved");
            h.update(std::process::id().to_be_bytes());
            h.update(
                UNOBSERVED
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    .to_be_bytes(),
            );
            h.update(format!("{:?}", std::time::SystemTime::now()).as_bytes());
        }
        h.update(b"empty-directories");
        for name in empty {
            feed(&mut h, &name);
            if let Some(m) = manifest.as_deref_mut() {
                m.insert(
                    format!("{}/", String::from_utf8_lossy(&name)),
                    "empty".to_owned(),
                );
            }
        }
        Ok((hex(h), flags))
    })
}
const WALK_LIMIT: usize = 300_000;
/// Paths of directories with no entries, outside the private stores and nested
/// repositories (those are observed by their own pass). `false` when the walk could
/// not be completed.
///
/// A large folder (a dependency folder alone can hold most of a project's entries) is
/// walked by several threads. A complete walk has one answer whatever the order; an
/// incomplete one (an unreadable directory) is the ordered walk's, which is then made
/// and returned. A walk over `WALK_LIMIT` entries is incomplete with no directories
/// named, whichever walk meets the bound: it is never walked a second time.
///
/// `pruned` directories (wholly Git-ignored ones, relative to `root`) are not entered:
/// ignored content is not deliverable, and a dependency or build folder can hold most
/// of a project's entries.
fn empty_directories(root: &Path, pruned: Option<&BTreeSet<Vec<u8>>>) -> (BTreeSet<Vec<u8>>, bool) {
    match empty_directories_parallel(root, WALKERS, pruned) {
        Ok(found) => (found, true),
        Err(true) => (BTreeSet::new(), false),
        Err(false) => empty_directories_ordered(root, pruned),
    }
}
/// Threads of one parallel empty-directory walk.
const WALKERS: usize = 8;
/// The entries `empty_directories_ordered` visits, read by `threads` threads at once.
/// `Err(true)` when they exceed `WALK_LIMIT`, `Err(false)` when the walk is otherwise
/// incomplete.
fn empty_directories_parallel(
    root: &Path,
    threads: usize,
    pruned: Option<&BTreeSet<Vec<u8>>>,
) -> Result<BTreeSet<Vec<u8>>, bool> {
    use std::os::unix::ffi::OsStrExt;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Condvar, Mutex,
    };
    type Job = (std::path::PathBuf, Vec<u8>);
    // Directories waiting to be read, and how many are being read right now.
    let queue: Mutex<(Vec<Job>, usize)> = Mutex::new((vec![(root.to_owned(), Vec::new())], 0));
    let ready = Condvar::new();
    let seen = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let over = AtomicBool::new(false);
    let found = Mutex::new(BTreeSet::new());
    // Set under the queue lock, so a walker about to wait always sees it or is woken.
    let fail = || {
        let _queue = queue.lock();
        failed.store(true, Ordering::SeqCst);
        ready.notify_all();
    };
    // The next directory to read; None once none is left or the walk failed.
    let next = || -> Option<Job> {
        let mut q = queue.lock().ok()?;
        loop {
            if failed.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(job) = q.0.pop() {
                q.1 += 1;
                return Some(job);
            }
            if q.1 == 0 {
                return None;
            }
            q = ready.wait(q).ok()?;
        }
    };
    // One read directory's subdirectories join the queue.
    let done = |children: Vec<Job>| match queue.lock() {
        Ok(mut q) => {
            q.0.extend(children);
            q.1 -= 1;
            ready.notify_all();
        }
        Err(_) => {
            failed.store(true, Ordering::SeqCst);
            ready.notify_all();
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                let mut empty = Vec::new();
                while let Some((dir, rel)) = next() {
                    let mut children = Vec::new();
                    let read = (|| -> Option<bool> {
                        let mut any = false;
                        for entry in std::fs::read_dir(&dir).ok()? {
                            let entry = entry.ok()?;
                            any = true;
                            if seen.fetch_add(1, Ordering::SeqCst) >= WALK_LIMIT {
                                over.store(true, Ordering::SeqCst);
                                return None;
                            }
                            let name = entry.file_name();
                            if !entry.file_type().ok()?.is_dir() {
                                continue;
                            }
                            if [".git", ".cortex", ".cortex-dev"]
                                .iter()
                                .any(|n| name.as_bytes() == n.as_bytes())
                            {
                                continue;
                            }
                            let mut child = rel.clone();
                            if !child.is_empty() {
                                child.push(b'/');
                            }
                            child.extend_from_slice(name.as_bytes());
                            if pruned.is_some_and(|p| p.contains(&child)) {
                                continue;
                            }
                            let path = entry.path();
                            // A nested checkout is observed by its own pass.
                            if path.join(".git").symlink_metadata().is_ok() {
                                continue;
                            }
                            children.push((path, child));
                        }
                        Some(any)
                    })();
                    let Some(any) = read else {
                        fail();
                        break;
                    };
                    if !any && !rel.is_empty() {
                        empty.push(rel);
                    }
                    done(children);
                }
                match found.lock() {
                    Ok(mut found) => found.extend(empty),
                    Err(_) => fail(),
                }
            });
        }
    });
    if failed.load(Ordering::SeqCst) {
        return Err(over.load(Ordering::SeqCst));
    }
    found.into_inner().map_err(|_| false)
}
/// The single ordered walk: its result, complete or not, is the observation's
/// (over `WALK_LIMIT`, incomplete with no directories named).
fn empty_directories_ordered(
    root: &Path,
    pruned: Option<&BTreeSet<Vec<u8>>>,
) -> (BTreeSet<Vec<u8>>, bool) {
    use std::os::unix::ffi::OsStrExt;
    let mut found = BTreeSet::new();
    let mut seen = 0usize;
    let mut pending = vec![(root.to_owned(), Vec::<u8>::new())];
    while let Some((dir, rel)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return (found, false);
        };
        let mut any = false;
        for entry in entries {
            let Ok(entry) = entry else {
                return (found, false);
            };
            any = true;
            seen += 1;
            if seen > WALK_LIMIT {
                return (BTreeSet::new(), false);
            }
            let name = entry.file_name();
            let Ok(kind) = entry.file_type() else {
                return (found, false);
            };
            if !kind.is_dir() {
                continue;
            }
            if [".git", ".cortex", ".cortex-dev"]
                .iter()
                .any(|n| name.as_bytes() == n.as_bytes())
            {
                continue;
            }
            let mut child = rel.clone();
            if !child.is_empty() {
                child.push(b'/');
            }
            child.extend_from_slice(name.as_bytes());
            if pruned.is_some_and(|p| p.contains(&child)) {
                continue;
            }
            let path = entry.path();
            // A nested checkout is observed by its own pass.
            if path.join(".git").symlink_metadata().is_ok() {
                continue;
            }
            pending.push((path, child));
        }
        if !any && !rel.is_empty() {
            found.insert(rel);
        }
    }
    (found, true)
}
/// Metadata-only digest of Git-ignored files (type, mode, size, mtime, link target),
/// in byte order of their names. `None` when they cannot be observed (an invalid path,
/// a metadata lookup error, or past `TIME_LIMIT`); a review waiver then cannot be granted.
/// Ignored files are not deliverable content, so this never makes the deliverable
/// observation incomplete.
///
/// Git lists them already sorted, so the digest is computed over its listing as it
/// stands, without collecting the names; a listing in any other order (or with a
/// repeat) is sorted once, so the same files always give the same digest. An entry
/// that vanished since the listing is recorded as missing. Names inside the private
/// stores or repository metadata are skipped, as everywhere else.
fn ignored_tree(root: &Path, listing: &[u8]) -> Option<String> {
    ignored_tree_within(root, listing, TIME_LIMIT)
}
fn ignored_tree_within(root: &Path, listing: &[u8], time_limit: Duration) -> Option<String> {
    let names = || listing.split(|c| *c == 0).filter(|p| !p.is_empty());
    let mut previous: Option<&[u8]> = None;
    let sorted = names().all(|name| {
        let ordered = previous.is_none_or(|p| p < name);
        previous = Some(name);
        ordered
    });
    let dir = File::open(root).ok()?;
    let started = std::time::Instant::now();
    let mut h = <sha2::Sha256 as sha2::Digest>::new();
    if sorted {
        for name in names() {
            ignored_entry(&mut h, &dir, name, started, time_limit)?;
        }
    } else {
        let mut all: Vec<&[u8]> = names().collect();
        all.sort_unstable();
        all.dedup();
        for name in all {
            ignored_entry(&mut h, &dir, name, started, time_limit)?;
        }
    }
    Some(hex(h))
}
fn ignored_entry(
    h: &mut sha2::Sha256,
    dir: &File,
    name: &[u8],
    started: std::time::Instant,
    time_limit: Duration,
) -> Option<()> {
    use sha2::Digest;
    if started.elapsed() >= time_limit {
        return None;
    }
    let name = std::str::from_utf8(name).ok()?;
    if name.starts_with('/') || name.split('/').any(|c| c == "..") {
        return None;
    }
    if name
        .split('/')
        .any(|c| [".cortex", ".cortex-dev", ".git"].contains(&c))
    {
        return Some(());
    }
    let seen = lookup(dir, name, false).ok()?;
    feed(h, name.as_bytes());
    h.update([seen.kind]);
    h.update(seen.mode.to_be_bytes());
    h.update(seen.size.to_be_bytes());
    h.update(seen.mtime.0.to_be_bytes());
    h.update(seen.mtime.1.to_be_bytes());
    feed(h, &seen.target);
    Some(())
}
