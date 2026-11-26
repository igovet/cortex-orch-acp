//! A disposable index and object directory for content-containment inspection.
use super::{git::Git, native::*};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
fn link_bytes(root: &Path, name: &str) -> Result<Vec<u8>> {
    use std::{
        ffi::CString,
        fs::File,
        os::fd::{AsRawFd, FromRawFd},
    };
    relative(name)?;
    let mut folder = File::open(root)?;
    let parts = name.split('/').collect::<Vec<_>>();
    for part in &parts[..parts.len() - 1] {
        let part = CString::new(*part).map_err(|_| refuse("invalid_path", "Invalid path."))?;
        let fd = unsafe {
            libc::openat(
                folder.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if fd < 0 {
            return Err(refuse(
                "containment_unavailable",
                "A path parent cannot be observed.",
            ));
        }
        folder = unsafe { File::from_raw_fd(fd) };
    }
    let leaf = CString::new(*parts.last().unwrap())
        .map_err(|_| refuse("invalid_path", "Invalid path."))?;
    let mut bytes = vec![0u8; 4097];
    let n = unsafe {
        libc::readlinkat(
            folder.as_raw_fd(),
            leaf.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if n < 0 || n > 4096 {
        return Err(refuse(
            "containment_unavailable",
            "Link content is unavailable.",
        ));
    }
    bytes.truncate(n as usize);
    Ok(bytes)
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let p = std::env::temp_dir().join(format!(
                "cortex-facts-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&p) {
                Ok(()) => {
                    let result = Self(p);
                    fs::create_dir(result.0.join("objects"))?;
                    return Ok(result);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(refuse(
            "inspection_unavailable",
            "Private inspection storage is unavailable.",
        ))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Shadow<'a> {
    scratch: Scratch,
    path: &'a Path,
    objects: String,
    configs: Vec<String>,
}
impl Shadow<'_> {
    async fn run(&self, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>> {
        let mut cmd = Command::new(super::git::git_program());
        // No inherited Git/askpass/loader state. Repo filters are disabled for every
        // command, including hash-object, and object/index writes are private.
        cmd.env_clear();
        for name in ["PATH", "HOME", "LANG", "XDG_CONFIG_HOME"] {
            if let Some(value) = std::env::var_os(name) {
                cmd.env(name, value);
            }
        }
        cmd.args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.pager=cat",
            "-c",
            "core.quotepath=false",
        ])
        .args(&self.configs)
        .args(args)
        .current_dir(self.path)
        .env("GIT_INDEX_FILE", self.scratch.0.join("index"))
        .env("GIT_OBJECT_DIRECTORY", self.scratch.0.join("objects"))
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &self.objects)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap().take(2_000_001);
        let run = async {
            let writer = async move {
                if let Some(data) = input {
                    stdin.write_all(data).await?;
                }
                stdin.shutdown().await
            };
            let reader = async {
                let mut data = Vec::new();
                stdout.read_to_end(&mut data).await?;
                Ok::<_, std::io::Error>(data)
            };
            let (w, r, status) = tokio::join!(writer, reader, child.wait());
            w?;
            let bytes = r?;
            if !status?.success() || bytes.len() > 2_000_000 {
                return Err(refuse(
                    "containment_unavailable",
                    "Local content inspection is unavailable.",
                ));
            }
            Ok(bytes)
        };
        tokio::time::timeout(Duration::from_secs(8), run)
            .await
            .map_err(|_| {
                refuse(
                    "containment_unavailable",
                    "Local content inspection expired.",
                )
            })?
    }
}
pub async fn cross_branch(
    path: &Path,
    record: &Value,
    target: &str,
    git: &mut Git,
    facts: &mut Value,
) -> Result<()> {
    let Some(base) = record["baseRevision"]
        .as_str()
        .filter(|s| !s.is_empty() && !s.starts_with('-'))
    else {
        return Ok(());
    };
    if !git
        .success(
            path,
            &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
        )
        .await?
        || !git
            .out(path, &["diff", "--name-only", "--diff-filter=U"])
            .await?
            .trim()
            .is_empty()
    {
        return Ok(());
    }
    let objects = git
        .out(
            path,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
            ],
        )
        .await?
        .trim()
        .to_owned();
    let mut configs = Vec::new();
    // git config exits 1 when no filters exist; success() probes that case without
    // turning a missing optional filter into a failed repository read.
    if git
        .success(path, &["config", "--get-regexp", r"^filter\."])
        .await?
    {
        for line in git
            .out(
                path,
                &[
                    "config",
                    "--null",
                    "--name-only",
                    "--get-regexp",
                    r"^filter\.",
                ],
            )
            .await?
            .split('\0')
        {
            if let Some(rest) = line.strip_prefix("filter.") {
                if let Some((driver, _)) = rest.rsplit_once('.') {
                    for (name, value) in [
                        ("clean", ""),
                        ("smudge", ""),
                        ("process", ""),
                        ("required", "false"),
                    ] {
                        configs.extend(["-c".into(), format!("filter.{driver}.{name}={value}")]);
                    }
                }
            }
        }
    }
    let shadow = Shadow {
        scratch: Scratch::new()?,
        path,
        objects,
        configs,
    };
    shadow.run(&["read-tree", "HEAD"], None).await?;
    let mut names = std::collections::BTreeSet::new();
    for args in [
        &["diff", "--name-only", "-z", "HEAD"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
    ] {
        for n in git
            .out(path, args)
            .await?
            .split('\0')
            .filter(|s| !s.is_empty() && !RESERVED.contains(&s.split('/').next().unwrap_or("")))
        {
            names.insert(n.to_owned());
        }
    }
    for name in names {
        relative(&name)?;
        let file = path.join(&name);
        let info = match fs::symlink_metadata(&file) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                shadow
                    .run(&["update-index", "--force-remove", "--", &name], None)
                    .await?;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        use std::os::unix::fs::PermissionsExt;
        let (bytes, mode) = if info.file_type().is_symlink() {
            (link_bytes(path, &name)?, "120000")
        } else {
            if !info.is_file() || info.len() > 2_000_000 {
                return Ok(());
            }
            (
                super::task_facts::safe_bytes(path, &name, 2_000_000)?,
                if info.permissions().mode() & 0o100 != 0 {
                    "100755"
                } else {
                    "100644"
                },
            )
        };
        let hash = shadow
            .run(&["hash-object", "-w", "--stdin"], Some(&bytes))
            .await?;
        let hash = std::str::from_utf8(&hash)
            .map_err(|_| refuse("containment_unavailable", "Invalid object identity."))?
            .trim();
        shadow
            .run(
                &["update-index", "--add", "--cacheinfo", mode, hash, &name],
                None,
            )
            .await?;
    }
    let patch = shadow
        .run(
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--binary",
                "--full-index",
                base,
                "--",
                ".",
                ":(exclude).cortex",
                ":(exclude).cortex-dev",
                ":(exclude).cortex-dev-owner.json",
                ":(exclude).cortex-workspace-dev.json",
                ":(exclude).cortex-workspace.json",
            ],
            None,
        )
        .await?;
    if !patch.is_empty() {
        shadow
            .run(&["read-tree", &format!("refs/heads/{target}")], None)
            .await?;
        shadow
            .run(
                &[
                    "apply",
                    "--reverse",
                    "--check",
                    "--cached",
                    "--whitespace=nowarn",
                ],
                Some(&patch),
            )
            .await?;
    }
    facts["alreadyInTarget"] = serde_json::json!(true);
    if patch.is_empty() {
        // The task branch carries nothing beyond its base: the result is the report.
        facts["noChanges"] = serde_json::json!(true);
        facts["pendingPaths"] = serde_json::json!([]);
    }
    Ok(())
}
