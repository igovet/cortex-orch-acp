"""Test-only verification launch: real subprocesses without namespace nesting."""
from contextlib import contextmanager


@contextmanager
def verification_command(root, scratch, argv):
    # Literal argv stays literal, even with spaces or shell metacharacters.
    # The shell only sets cwd/umask; it never evaluates the check's arguments.
    command = [
        "/usr/bin/env", f"HOME={scratch}", f"TMPDIR={scratch}",
        f"TMP={scratch}", f"TEMP={scratch}", "PYTHONDONTWRITEBYTECODE=1",
        f"PYTEST_ADDOPTS=-o cache_dir={scratch}/pytest-cache",
        "/bin/sh", "-c", 'umask 022; cd "$1" || exit; shift; exec "$@"',
        "cortex-test-verification", str(root), *argv,
    ]
    yield command, ()


def project_git_command(root, scratch, argv, metadata):
    with verification_command(root, scratch, argv) as (command, _):
        return ["/usr/bin/env", f"GIT_DIR={metadata[0]}", f"GIT_WORK_TREE={root}",
                "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL=/dev/null",
                "GIT_TERMINAL_PROMPT=0", *command]


def unjailed_gateway_argv(command, fds):
    # Keep the real gateway's environment, literal argv and cwd; replace only
    # its final OS launch, explicitly opted into by receipt/recovery tests.
    if command[0].rsplit("/", 1)[-1] != "bwrap":
        return command, tuple(fds)
    split = command.index("--")
    cwd = command[command.index("--chdir") + 1]
    env = []
    for index, arg in enumerate(command[:split]):
        if arg == "--setenv":
            env.append(f"{command[index + 1]}={command[index + 2]}")
    return ["/usr/bin/env", *env, "/bin/sh", "-c",
            'cd "$1" || exit; shift; exec "$@"', "cortex-test-command", cwd,
            *command[split + 1:]], tuple(fds)


def sandboxed_git(root, args, timeout=20, ok_codes=(0,)):
    """Real read-only git for revision/skill tests, without namespace nesting."""
    import subprocess
    import tempfile
    from cortex_runtime.delegation.revision import git_metadata_dirs
    metadata = git_metadata_dirs(root)
    if metadata is None:
        return None
    argv = ["/usr/bin/git", "--no-optional-locks", "-c", "core.fsmonitor=false",
            "-c", "core.hooksPath=/dev/null", *args]
    try:
        with tempfile.TemporaryDirectory(prefix="cortex-test-git-") as scratch:
            done = subprocess.run(project_git_command(root, scratch, argv, metadata),
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  env={"PATH": "/usr/bin:/bin"}, timeout=timeout)
    except Exception:
        return None
    return done.stdout if done.returncode in ok_codes else None


def bwrap_namespace_reason(binary, run):
    """Only known namespace denials are capability skips; other errors fail."""
    if binary is None:
        return "bubblewrap is not installed"
    result = run([binary, "--unshare-user", "--ro-bind", "/", "/", "--", "true"],
                 capture_output=True, text=True, timeout=10)
    if result.returncode == 0:
        return None
    if ("No permissions to create a new namespace" in result.stderr or
            "Creating new namespace failed: Operation not permitted" in result.stderr):
        return "bwrap cannot create user namespace in this environment (nested sandbox)"
    raise AssertionError(f"bubblewrap capability probe failed unexpectedly: {result.stderr}")
