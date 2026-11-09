"""Build channels must fail closed and never share durable or runtime locations."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[1]


def builder():
    spec = importlib.util.spec_from_file_location("channel_builder", ROOT / "scripts/build_workspace_desktop.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("authorization,accepted", [
    ("github-main-clean", True), ("local-owner-test", True),
    (None, False), ("", False), ("local-owner-test-extra", False), ("other", False),
])
def test_rust_release_build_guard(tmp_path, authorization, accepted):
    import shutil
    import pwd
    # The suite replaces HOME, but the installed toolchain is account-owned.
    # Match the runner's PATH/CARGO_HOME lookup without treating fixture HOME
    # as a Rust installation; rustup also needs its real toolchain root.
    account = Path(pwd.getpwuid(os.getuid()).pw_dir)
    cargo_home = Path(os.environ.get("CARGO_HOME", str(account / ".cargo")))
    rustc = shutil.which("rustc") or shutil.which(str(cargo_home / "bin/rustc"))
    assert rustc, "The installed Rust compiler is required to execute the release guard"
    build_env = dict(os.environ)
    build_env.setdefault("RUSTUP_HOME", str(account / ".rustup"))
    executable = tmp_path / "build-guard"
    subprocess.run([rustc, str(ROOT / "crates/workspace-protocol/build.rs"),
                    "-o", str(executable)], check=True, timeout=30, env=build_env)
    env = dict(os.environ, CORTEX_WORKSPACE_CHANNEL="release")
    env.pop("CORTEX_WORKSPACE_RELEASE_AUTHORIZED", None)
    if authorization is not None:
        env["CORTEX_WORKSPACE_RELEASE_AUTHORIZED"] = authorization
    result = subprocess.run([str(executable)], env=env, capture_output=True, text=True, timeout=10)
    assert (result.returncode == 0) is accepted
    if accepted:
        assert "cargo:rustc-env=CORTEX_WORKSPACE_CHANNEL=release" in result.stdout
    # Dev does not require a release authorization.
    env["CORTEX_WORKSPACE_CHANNEL"] = "dev"
    assert subprocess.run([str(executable)], env=env, capture_output=True, timeout=10).returncode == 0


def test_channels_are_disjoint():
    table = json.loads((ROOT / "crates/workspace-protocol/channels.json").read_text())
    assert set(table) == {"dev", "release"}
    dev, release = table["dev"], table["release"]
    for key in ("package", "identifier", "title", "desktop_binary", "service_binary", "unit",
                "runtime_namespace", "config_namespace", "state_namespace"):
        assert dev[key] != release[key], key
    for base, namespace, leaf in (("/run/user/1000", "runtime_namespace", "service.sock"),
                                  ("/run/user/1000", "runtime_namespace", "service.lock"),
                                  ("/state", "state_namespace", "projects.json"),
                                  ("/config", "config_namespace", "cortex/connections.json"),
                                  ("/state", "state_namespace", "cortex/catalog.sqlite3")):
        assert Path(base) / dev[namespace] / leaf != Path(base) / release[namespace] / leaf


def test_default_is_dev_and_release_is_explicit():
    assert builder().parse_args([]).release is False
    assert builder().parse_args(["--release"]).release is True


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def test_release_requires_github_main_and_clean_tree(tmp_path):
    root = tmp_path / "checkout"
    root.mkdir()
    git(root, "init", "-b", "main")
    git(root, "config", "user.name", "Fixture")
    git(root, "config", "user.email", "fixture@example.invalid")
    (root / "source").write_text("source")
    git(root, "add", "source")
    git(root, "commit", "-m", "fixture")
    module = builder()
    with pytest.raises(SystemExit, match="GitHub"):
        module.require_release(root, {})
    env = {"GITHUB_ACTIONS": "true", "GITHUB_REF": "refs/heads/main", "GITHUB_EVENT_NAME": "workflow_dispatch"}
    module.require_release(root, env)
    git(root, "checkout", "-b", "feature")
    with pytest.raises(SystemExit, match="main"):
        module.require_release(root, env)
    git(root, "checkout", "main")
    (root / "untracked").write_text("dirty")
    with pytest.raises(SystemExit, match="clean"):
        module.require_release(root, env)


def test_debian_payloads_have_no_shared_file(tmp_path, monkeypatch, request):
    # Layout unit check only; payload integrity and actual adapters have separate checks.
    from test_workspace_packaging import STAMP, engine_fixture, voice_fixture
    # A private builder umask must not reach the package: dpkg-deb refuses a 0700
    # control directory, and every installed path is read by the owner, not root.
    previous = os.umask(0o077)
    request.addfinalizer(lambda: os.umask(previous))
    spec = importlib.util.spec_from_file_location("channel_package", ROOT / "packaging/container/package_channel.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    target = tmp_path / "target/release"
    target.mkdir(parents=True)
    for name in ("cortex-workspace-desktop", "workspace-service"):
        (target / name).write_text("fixture executable")
    engines = engine_fixture(tmp_path)
    voice_fixture(tmp_path)
    payload = tmp_path / "fixture.py"
    payload.write_text("fixture runtime")
    monkeypatch.setattr(module, "payload_files", lambda root: [("fixture.py", payload)])
    layouts = {}
    for channel in ("dev", "release"):
        monkeypatch.setenv("CORTEX_WORKSPACE_RELEASE_AUTHORIZED", "github-main-clean")
        output = tmp_path / channel
        module.package(ROOT, tmp_path / "target", output, channel, engines=engines, stamp=STAMP)
        archive = next(output.glob("*.deb"))
        listing = subprocess.check_output(["dpkg-deb", "-c", str(archive)], text=True).splitlines()
        assert listing and all(line[7] == "r" for line in listing), [l for l in listing if l[7] != "r"][:3]
        stage = tmp_path / ("unpacked-" + channel)
        subprocess.run(["dpkg-deb", "--extract", str(archive), str(stage)], check=True)
        layouts[channel] = {str(p.relative_to(stage)) for p in stage.rglob("*") if p.is_file()}
        table = module.channel(ROOT, channel)
        unit = (stage / "usr/lib/systemd/user" / table["unit"]).read_text()
        assert table["service_binary"] in unit
        assert "CORTEX_WORKSPACE_CHECKOUT" not in unit
        assert "/usr/lib/" + table["package"] + "/runtime" in unit
        assert "EnvironmentFile" not in unit
        assert subprocess.check_output(["dpkg-deb", "--field", str(archive), "Package"], text=True).strip() == table["package"]
    assert not layouts["dev"] & layouts["release"]


def test_dev_store_beside_release_never_opens_or_migrates_it(tmp_path, monkeypatch):
    from cortex_runtime.project_storage import project_store_directory
    from cortex_runtime.store import Store, project_draft_directory, project_task_directory
    release = tmp_path / ".cortex"
    release.mkdir(mode=0o700)
    canary = release / "cortex.sqlite3"
    canary.write_bytes(b"release store must not be read or migrated")
    before = (canary.read_bytes(), canary.stat().st_mtime_ns)
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "dev")
    directory = project_store_directory(tmp_path)
    assert directory == tmp_path / ".cortex-dev"
    store = Store(directory, project_root=tmp_path)
    assert store.path.is_file()
    assert project_draft_directory(tmp_path).parent == directory
    assert project_task_directory(tmp_path) == directory
    assert (canary.read_bytes(), canary.stat().st_mtime_ns) == before
    assert sorted(p.name for p in release.iterdir()) == ["cortex.sqlite3"]
    from cortex_runtime.contracts import StoreError
    with pytest.raises(StoreError, match="project_storage_mismatch"):
        Store(release, project_root=tmp_path)
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "release")
    assert project_store_directory(tmp_path) == release


def test_dev_never_imports_legacy_release_storage(tmp_path, monkeypatch):
    from cortex_runtime.project_storage import migrate_legacy_project_storage
    legacy = tmp_path / ".codex" / "cortex"
    legacy.mkdir(parents=True, mode=0o700)
    (legacy / "cortex.sqlite3").write_bytes(b"legacy release canary")
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "dev")
    assert migrate_legacy_project_storage(tmp_path) is False
    assert (legacy / "cortex.sqlite3").read_bytes() == b"legacy release canary"
    assert not (tmp_path / ".cortex-dev").exists()


def test_dev_format11_inspection_and_migration_stay_in_dev(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.store import Store
    release = tmp_path / ".cortex"
    release.mkdir(mode=0o700)
    (release / "cortex.sqlite3").write_bytes(b"untouched release")
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "dev")
    store = Store(tmp_path / ".cortex-dev", project_root=tmp_path)
    with store.connection() as db:
        db.execute("PRAGMA user_version=11")
        db.commit()
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    before = store.path.read_bytes()
    with pytest.raises(StoreError, match="legacy_storage_migration_required"):
        Store(store.directory, initialize=False, project_root=tmp_path)
    assert store.path.read_bytes() == before
    assert not (store.directory / ".migration-backups").exists()
    Store(store.directory, project_root=tmp_path)
    with store.connection() as db:
        assert db.execute("PRAGMA user_version").fetchone()[0] == 12
    assert len(list((store.directory / ".migration-backups").glob("*.bak"))) == 1
    assert list(release.iterdir()) == [release / "cortex.sqlite3"]
    assert (release / "cortex.sqlite3").read_bytes() == b"untouched release"


def test_dev_private_paths_and_effective_gitignore(tmp_path, monkeypatch):
    from cortex_runtime.orchestration.ignore_guard import protected_content
    from cortex_runtime.orchestration.project_paths import permitted_path
    from cortex_runtime.workspace_api.task_options import exclude_helpers
    git(tmp_path, "init", "-b", "main")
    monkeypatch.setenv("CORTEX_WORKSPACE_CHANNEL", "dev")
    (tmp_path / ".gitignore").write_text(protected_content("!.cortex-dev/\n"))
    exclude_helpers(tmp_path)
    for name in (".cortex-dev", ".cortex"):
        assert not permitted_path(name + "/cortex.sqlite3")
        assert git(tmp_path, "check-ignore", "--no-index", name + "/cortex.sqlite3")
    assert not permitted_path(".cortex-dev-owner.json")
    assert not permitted_path(".cortex-workspace-dev.json")


def test_default_tauri_identity_matches_channel():
    channel = json.loads((ROOT / "crates/workspace-protocol/channels.json").read_text())["dev"]
    config = json.loads((ROOT / "apps/cortex-workspace-desktop/src-tauri/tauri.conf.json").read_text())
    assert config["productName"] == channel["title"]
    assert config["identifier"] == channel["identifier"]
