"""No-paid container builder argument and credential-boundary checks."""
import importlib.util
import os
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
STAMP = "20261106060000000000"


def builder():
    spec = importlib.util.spec_from_file_location("workspace_builder", ROOT / "scripts/build_workspace_desktop.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("args", [
    ["--engine", "sudo"], ["--jobs", "0"], ["--jobs", "-1"],
    ["--output", "/home"], ["--env", "TOKEN"], ["--volume", "/home:/home"],
    ["--image", "unreviewed"], ["--jobs", "not-a-number"],
    ["--build-stamp", "20260230000000000000"], ["--build-stamp", "20261106"],
    ["--build-stamp", "20261106060000000000\n"],
    ["--resume", "../escape"], ["--resume", "/home"], ["--jobs", "17"],
])
def test_rejects_unsafe_or_invalid_arguments(args):
    with pytest.raises(SystemExit) as error:
        builder().parse_args(args)
    assert error.value.code == 2


def test_defaults_and_podman():
    assert builder().parse_args([]).engine == "docker"
    assert builder().parse_args(["--engine", "podman", "--jobs", "2"]).jobs == 2
    assert builder().parse_args(["--test-workspace"]).test_workspace is True
    assert builder().parse_args(["--check-install"]).check_install is True
    assert builder().parse_args(["--check-installed", "--prior-archive", "prior.deb"]).check_installed
    for args in (["--check-installed"], ["--prior-archive", "prior.deb"],
                 ["--check-installed", "--prior-archive", "prior.deb", "--engine", "podman"]):
        with pytest.raises(SystemExit):
            builder().parse_args(args)


def test_snapshot_excludes_credentials_caches_and_symlinks(tmp_path):
    source = tmp_path / "repo"
    source.mkdir()
    inputs = {
        "Cargo.toml": "[workspace]",
        "apps/cortex-workspace-desktop/src/main.tsx": "source",
        "apps/cortex-workspace-desktop/.env": "private",
        "apps/cortex-workspace-desktop/.npmrc": "private",
        "apps/cortex-workspace-desktop/node_modules/pkg/index.js": "private",
        ".git/config": "private",
        ".cortex/cortex.sqlite3": "private",
        ".ssh/id_rsa": "private",
        "unrelated.json": "private",
    }
    for name, content in inputs.items():
        path = source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    (source / "apps/cortex-workspace-desktop/src/link.ts").symlink_to(source / ".ssh/id_rsa")
    dest = tmp_path / "input"
    builder().snapshot(source, dest)
    files = sorted(str(p.relative_to(dest)) for p in dest.rglob("*") if p.is_file())
    assert files == ["Cargo.toml", "apps/cortex-workspace-desktop/src/main.tsx"]
    assert all("private" not in p.read_text() for p in dest.rglob("*") if p.is_file())


def test_snapshot_never_follows_an_ancestor_link(tmp_path):
    source = tmp_path / "repo"
    source.mkdir()
    external = tmp_path / "private"
    (external / "cortex-workspace-desktop/src").mkdir(parents=True)
    (external / "cortex-workspace-desktop/src/secret.ts").write_text("private")
    (source / "apps").symlink_to(external, target_is_directory=True)
    destination = tmp_path / "input"
    builder().snapshot(source, destination)
    assert not list(destination.rglob("*.ts"))


def test_snapshot_contains_complete_installed_runtime(tmp_path):
    module = builder()
    module.snapshot(ROOT, tmp_path / "snapshot")
    import sys
    sys.path.insert(0, str(ROOT / "packaging/container"))
    from runtime_payload import payload_files
    assert (tmp_path / "snapshot/packaging/container/debian_version.py").is_file()
    for relative, source in payload_files(ROOT):
        assert (tmp_path / "snapshot" / relative).read_bytes() == source.read_bytes()


def test_commands_never_forward_host_environment_or_home(tmp_path, monkeypatch):
    module = builder()
    monkeypatch.setenv("AWS_SECRET_ACCESS_KEY", "do-not-forward")
    monkeypatch.setenv("DOCKER_HOST", "do-not-forward")
    monkeypatch.setenv("NPM_TOKEN", "do-not-forward")
    output = tmp_path / "output"
    output.mkdir()
    env = module.command_environment(output)
    assert set(env) == {"PATH", "HOME", "DOCKER_CONFIG"}
    assert env["HOME"] != os.environ.get("HOME")
    assert "do-not-forward" not in str(env)
    run = module.run_command("docker", output, 2, stamp=STAMP)
    assert "--privileged" not in run and "--env-file" not in run
    assert "--user" in run and run[run.index("--user") + 1] == f"{os.getuid()}:{os.getgid()}"
    mounts = [run[i + 1] for i, value in enumerate(run) if value == "--mount"]
    assert len(mounts) == 2
    assert str(ROOT) not in str(mounts)
    assert "readonly" in mounts[0]
    assert "HOME=/build/home" in run
    assert "CORTEX_WORKSPACE_BUILD_STAMP=" + STAMP in run
    release = module.run_command("docker", output, 2, channel="release", stamp=STAMP)
    assert "CORTEX_WORKSPACE_BUILD_STAMP=" + STAMP in release
    assert "CORTEX_WORKSPACE_RELEASE_AUTHORIZED=github-main-clean" in release
    assert run[-1] == "2"
    assert module.run_command("docker", output, 2, True, stamp=STAMP)[-1] == "test"


def test_stamp_allocation_is_monotonic_and_resume_replays(tmp_path):
    module = builder()
    first, second, third = (tmp_path / name for name in ("one", "two", "three"))
    for path in (first, second, third):
        path.mkdir()
    # A future retained value exercises repeated/backward clock handling.
    future = "20991231235959999999"
    assert module.select_stamp(tmp_path, first, future) == future
    following = module.select_stamp(tmp_path, second)
    assert following == "21000101000000000000"
    assert module.select_stamp(tmp_path, first, future) == future
    # Reusing a compile cache is still a new package build unless explicitly replayed.
    resumed = module.select_stamp(tmp_path, first)
    assert resumed == "21000101000000000001"
    with pytest.raises(SystemExit, match="exceed"):
        module.select_stamp(tmp_path, first, following)
    with pytest.raises(SystemExit, match="exceed"):
        module.select_stamp(tmp_path, third, future)
    assert not (third / "build-stamp.txt").exists()


def test_concurrent_builds_reserve_distinct_increasing_stamps(tmp_path):
    from concurrent.futures import ThreadPoolExecutor
    module = builder()
    seed = tmp_path / "seed"
    seed.mkdir()
    module.select_stamp(tmp_path, seed, "20990101000000000000")
    outputs = [tmp_path / str(i) for i in range(8)]
    for output in outputs:
        output.mkdir()
    with ThreadPoolExecutor(max_workers=8) as pool:
        stamps = list(pool.map(lambda output: module.select_stamp(tmp_path, output), outputs))
    assert sorted(stamps) == [f"2099010100000000000{i}" for i in range(1, 9)]
    assert (tmp_path / "last-build-stamp.txt").read_text().strip() == max(stamps)


def test_stamp_state_rejects_symlinks(tmp_path):
    module = builder()
    output = tmp_path / "build"
    output.mkdir()
    victim = tmp_path / "victim"
    victim.write_text(STAMP)
    (output / "build-stamp.txt").symlink_to(victim)
    with pytest.raises(SystemExit, match="symlink"):
        module.select_stamp(tmp_path, output)
    (output / "build-stamp.txt").unlink()
    (tmp_path / "last-build-stamp.txt").symlink_to(victim)
    with pytest.raises(OSError):
        module.select_stamp(tmp_path, output)
    assert victim.read_text() == STAMP


def test_container_recipe_is_pinned_and_context_is_narrow():
    recipe = (ROOT / "packaging/container/Dockerfile").read_text()
    assert "FROM ubuntu:26.04@sha256:" in recipe
    for name in ("libwebkit2gtk-4.1-dev", "libssl-dev", "libayatana-appindicator3-dev", "librsvg2-dev", "libxdo-dev"):
        assert name in recipe
    assert "COPY . " not in recipe
    assert "24.14.1" in recipe
    script = (ROOT / "packaging/container/build.sh").read_text()
    assert '--build-stamp "$CORTEX_WORKSPACE_BUILD_STAMP"' in script
    # Desktop presentation consumes Paseo types, even without a local node_modules cache.
    assert script.index("npm --prefix integrations/paseo ci --ignore-scripts") < script.index(
        "npm --prefix apps/cortex-workspace-desktop run typecheck")


@pytest.mark.parametrize("other", ["--release", "--ci-dev"])
def test_local_release_is_mutually_exclusive(other):
    with pytest.raises(SystemExit) as error:
        builder().parse_args(["--local-release", other])
    assert error.value.code == 2


def test_local_release_arguments_and_installed_gate():
    args = builder().parse_args(["--local-release", "--jobs", "4", "--engine", "podman"])
    assert args.local_release and not args.release and not args.ci_dev
    assert args.jobs == 4 and args.engine == "podman"
    with pytest.raises(SystemExit):
        builder().parse_args(["--local-release", "--check-installed", "--prior-archive", "old.deb"])


def test_official_release_still_requires_github_clean_main(tmp_path, monkeypatch):
    module = builder()
    monkeypatch.delenv("GITHUB_ACTIONS", raising=False)
    with pytest.raises(SystemExit, match="workflow_dispatch"):
        module.main(["--release"])
    env = dict(GITHUB_ACTIONS="true", GITHUB_REF="refs/heads/main",
               GITHUB_EVENT_NAME="workflow_dispatch")
    monkeypatch.setattr(module.subprocess, "check_output", lambda *a, **k: "other")
    with pytest.raises(SystemExit, match="current branch main"):
        module.require_release(tmp_path, env)
    answers = iter(["main", " M tracked"])
    monkeypatch.setattr(module.subprocess, "check_output", lambda *a, **k: next(answers))
    with pytest.raises(SystemExit, match="clean tree"):
        module.require_release(tmp_path, env)
    answers = iter(["main", ""])
    monkeypatch.setattr(module.subprocess, "check_output", lambda *a, **k: next(answers))
    module.require_release(tmp_path, env)


def test_local_release_refuses_github(monkeypatch):
    monkeypatch.setenv("GITHUB_ACTIONS", "true")
    with pytest.raises(SystemExit, match="local owner testing"):
        builder().main(["--local-release"])


@pytest.mark.parametrize("flag,channel,marker", [
    ("--local-release", "release", "local-owner-test"),
    ("--release", "release", "github-main-clean"),
    (None, "dev", None),
])
def test_build_manifest_and_release_marker(tmp_path, monkeypatch, flag, channel, marker):
    import json
    module = builder()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    monkeypatch.delenv("GITHUB_ACTIONS", raising=False)
    (tmp_path / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.1"\n')
    recipe = tmp_path / "packaging/container"
    recipe.mkdir(parents=True)
    for name in ("Dockerfile", "build.sh"):
        (recipe / name).write_text("recipe")
    engines = tmp_path / "packaging/engines"
    engines.mkdir()
    (engines / "node-checksums.json").write_text("{}")
    monkeypatch.setattr(module, "snapshot", lambda root, dest: dest.mkdir())
    gates = []
    monkeypatch.setattr(module, "require_release", lambda *args: gates.append(args))
    commands = []

    def run(command, **kwargs):
        commands.append(command)
        if command[1] == "run":
            output = next((tmp_path / ".workspace-build").glob("build-*"))
            (output / "artifacts").mkdir()
            (output / "artifacts/package.deb").write_bytes(b"mock archive")

    monkeypatch.setattr(module.subprocess, "run", run)
    monkeypatch.setattr(module.subprocess, "check_output", lambda *a, **k: "sha256:mock")
    module.main([flag] if flag else [])
    output = next((tmp_path / ".workspace-build").glob("build-*"))
    manifest = json.loads((output / "manifest.json").read_text())
    assert manifest["channel"] == channel
    assert manifest["local_release"] == (flag == "--local-release")
    assert manifest["owner_handoff_eligible"] is False
    assert bool(gates) == (flag == "--release")
    run_command = commands[1]
    markers = [arg for arg in run_command if arg.startswith("CORTEX_WORKSPACE_RELEASE_AUTHORIZED=")]
    assert markers == (["CORTEX_WORKSPACE_RELEASE_AUTHORIZED=" + marker] if marker else [])


@pytest.mark.parametrize("kind,channel_args", [("dev", []), ("prod", ["--local-release"])])
def test_shell_wrappers_from_other_cwd(tmp_path, kind, channel_args):
    import json
    import shutil
    import subprocess
    repo = tmp_path / "linked worktree"
    scripts = repo / "scripts"
    scripts.mkdir(parents=True)
    wrapper = scripts / ("build-" + kind + "-deb.sh")
    shutil.copy2(ROOT / "scripts" / wrapper.name, wrapper)
    # A fake builder exercises invocation/output binding without a container or network.
    (scripts / "build_workspace_desktop.py").write_text(
        "import json, pathlib, sys\n"
        "args = sys.argv[1:]\n"
        "pathlib.Path('args.json').write_text(json.dumps(args))\n"
        "out = pathlib.Path('.workspace-build') / args[args.index('--resume') + 1]\n"
        "(out / 'artifacts').mkdir()\n"
        "(out / 'artifacts/package.deb').write_bytes(b'fixture')\n"
        "(out / 'manifest.json').write_text('{}')\n"
    )
    result = subprocess.run(["sh", str(wrapper), "--jobs", "4", "--engine", "podman"],
                            cwd=tmp_path, text=True, capture_output=True, check=True)
    args = json.loads((repo / "args.json").read_text())
    assert args[:len(channel_args)] == channel_args
    assert args[-4:] == ["--jobs", "4", "--engine", "podman"]
    archive = Path(result.stdout.strip())
    assert archive.read_bytes() == b"fixture"
    assert archive.parent.parent == repo / ".workspace-build" / ("local-" + kind)
    assert (archive.parent / "manifest.json").is_file()
    for forbidden in ("--release", "--ci-dev", "--local-release", "--resume=build-abcdefgh"):
        rejected = subprocess.run(["sh", str(wrapper), forbidden], cwd=tmp_path,
                                  text=True, capture_output=True)
        assert rejected.returncode == 2
    (scripts / "build_workspace_desktop.py").write_text("raise SystemExit(7)\n")
    failed = subprocess.run(["sh", str(wrapper)], cwd=tmp_path, text=True, capture_output=True)
    assert failed.returncode == 7 and failed.stdout == ""
