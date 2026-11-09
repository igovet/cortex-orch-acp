"""No-paid package layouts and manual two-channel release delivery policy."""
import configparser
import importlib.util
import io
import json
from pathlib import Path
import re
import subprocess
import urllib.error

import pytest

ROOT = Path(__file__).resolve().parents[1]
STAMP = "20261106060000000000"


def load(path):
    spec = importlib.util.spec_from_file_location("packaging_check", ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def workflow():
    # JSON is a YAML subset: using it avoids an extra test-only YAML dependency.
    return json.loads((ROOT / ".github/workflows/workspace-desktop-release.yml").read_text())


def test_local_release_version_cannot_pass_github_publication(monkeypatch):
    module = load("packaging/container/package_channel.py")
    identity = "1.0.6+sha256.0123456789abcdef"
    for marker in ("github-main-clean", "local-owner-test"):
        monkeypatch.setenv("CORTEX_WORKSPACE_RELEASE_AUTHORIZED", marker)
        version = module.package_version(identity, STAMP, "release")
        assert version.endswith(".local") is (marker == "local-owner-test")
        if marker == "local-owner-test":
            step = workflow()["jobs"]["publish"]["steps"][1]
            code = step["run"].split("\n", 1)[1].rsplit("\nPY", 1)[0]
            monkeypatch.setenv("VERSION", version)
            with pytest.raises(SystemExit, match="invalid package identity"):
                exec(compile(code, "<workflow-tag-check>", "exec"), {})
    for marker in ("", "other", "local-owner-test-extra"):
        monkeypatch.setenv("CORTEX_WORKSPACE_RELEASE_AUTHORIZED", marker)
        with pytest.raises(ValueError, match="authorization"):
            module.package_version(identity, STAMP, "release")
        assert module.package_version(identity, STAMP, "dev") == module.debian_version(identity, STAMP)
    monkeypatch.delenv("CORTEX_WORKSPACE_RELEASE_AUTHORIZED")
    with pytest.raises(ValueError, match="authorization"):
        module.package_version(identity, STAMP, "release")


def test_workflow_is_manual_two_channel_with_least_privilege():
    flow = workflow()
    assert flow["on"] == {"workflow_dispatch": {}}
    assert flow["concurrency"]["group"] == "workspace-desktop-release-${{ github.ref }}"
    assert flow["permissions"] == {"contents": "read"}
    assert flow["concurrency"]["cancel-in-progress"] is False
    assert set(flow["jobs"]) == {"build", "publish"}
    guard = "github.event_name == 'workflow_dispatch' && (github.ref == 'refs/heads/main' || github.ref == 'refs/heads/dev')"
    build, publish = flow["jobs"]["build"], flow["jobs"]["publish"]
    assert build["if"] == publish["if"] == guard
    assert build["permissions"] == {"contents": "read"}
    assert publish["permissions"] == {"contents": "write"}
    assert publish["needs"] == "build"
    steps = build["steps"]
    assert steps[0]["with"] == {"ref": "${{ github.sha }}", "persist-credentials": False}
    assert steps[1]["run"] == 'git checkout -B "${GITHUB_REF#refs/heads/}" "$GITHUB_SHA"'
    commands = [step.get("run", "") for step in steps]
    assert any("--release" in command and "--ci-dev" in command for command in commands)
    assert build["env"]["PACKAGE"] == "${{ github.ref == 'refs/heads/main' && 'cortex-workspace' || 'cortex-workspace-dev' }}"
    assert build["outputs"]["version"] == "${{ steps.release.outputs.version }}"
    gather = next(step for step in steps if step.get("id") == "release")
    assert 'artifacts/"${PACKAGE}"_*.deb' in gather["run"]
    assert '[ "$#" -eq 1 ]' in gather["run"]
    assert 'dpkg-deb -f "$1" Version' in gather["run"]
    assert 'sha256sum "$archive" > "$archive.sha256"' in gather["run"]
    assert "release-artifacts/*.deb.sha256" in publish["steps"][-1]["run"]
    assert "package-identity.json" in next(step["run"] for step in steps if step.get("id") == "identity")
    upload = next(step for step in steps if step.get("uses", "").startswith("actions/upload-artifact@"))
    assert upload["with"]["if-no-files-found"] == "error"
    assert upload["with"]["path"] == "release-artifacts/"
    for job in flow["jobs"].values():
        for step in job["steps"]:
            if "uses" in step:
                assert re.fullmatch(r"actions/[a-z-]+@[0-9a-f]{40}", step["uses"])
            assert "secrets." not in json.dumps(step)
            if "run" in step:
                subprocess.run(["bash", "-n"], input=step["run"], text=True, check=True)
    publish_step = publish["steps"][-1]
    assert publish_step["if"] == "steps.tag.outputs.exists == 'false'"
    assert '--target "$GITHUB_SHA"' in publish_step["run"]
    assert 'gh release create "$TAG_PREFIX$VERSION"' in publish_step["run"]
    assert publish["env"]["TAG_PREFIX"] == "${{ github.ref == 'refs/heads/main' && 'v' || 'dev-v' }}"
    assert "flags=(--latest)" in publish_step["run"]
    assert "flags=(--prerelease --latest=false)" in publish_step["run"]


@pytest.mark.parametrize("channel,branch", [("release", "main"), ("dev", "dev")])
@pytest.mark.parametrize("ref", ["refs/heads/main", "refs/heads/dev", "refs/heads/feature", "refs/tags/v1.0.6"])
@pytest.mark.parametrize("event", ["workflow_dispatch", "push", "pull_request"])
@pytest.mark.parametrize("actions", ["true", "false"])
def test_ci_builder_context_matrix(monkeypatch, channel, branch, ref, event, actions):
    module = load("scripts/build_workspace_desktop.py")
    monkeypatch.setattr(module.subprocess, "check_output",
                        lambda argv, **kwargs: branch if "--show-current" in argv else "")
    env = {"GITHUB_REF": ref, "GITHUB_EVENT_NAME": event, "GITHUB_ACTIONS": actions}
    if ref == "refs/heads/" + branch and event == "workflow_dispatch" and actions == "true":
        module.require_release(ROOT, env, channel)
    else:
        with pytest.raises(SystemExit, match="GitHub Actions workflow_dispatch"):
            module.require_release(ROOT, env, channel)


@pytest.mark.parametrize("channel,branch", [("release", "main"), ("dev", "dev")])
@pytest.mark.parametrize("current,dirty", [("feature", ""), ("", ""), (None, " M file")])
def test_ci_builder_requires_matching_clean_branch(monkeypatch, channel, branch, current, dirty):
    module = load("scripts/build_workspace_desktop.py")
    monkeypatch.setattr(module.subprocess, "check_output",
                        lambda argv, **kwargs: (branch if current is None else current)
                        if "--show-current" in argv else dirty)
    with pytest.raises(SystemExit, match="current branch|clean tree"):
        module.require_release(ROOT, {"GITHUB_ACTIONS": "true", "GITHUB_EVENT_NAME": "workflow_dispatch",
                                     "GITHUB_REF": "refs/heads/" + branch}, channel)


def test_local_dev_default_and_exclusive_ci_channels():
    module = load("scripts/build_workspace_desktop.py")
    args = module.parse_args([])
    assert not args.release and not args.ci_dev
    with pytest.raises(SystemExit):
        module.parse_args(["--release", "--ci-dev"])


@pytest.mark.parametrize("status,expected", [(200, "true"), (404, "false"), (401, None), (403, None), (500, None)])
@pytest.mark.parametrize("prefix", ["v", "dev-v"])
def test_tag_lookup_never_publishes_on_ambiguous_failure(tmp_path, monkeypatch, status, expected, prefix):
    step = workflow()["jobs"]["publish"]["steps"][1]
    code = step["run"].split("\n", 1)[1].rsplit("\nPY", 1)[0]
    output = tmp_path / "output"
    for key, value in {
        "VERSION": "1.0.6+sha256.0123456789abcdef", "GH_TOKEN": "fixture-only", "TAG_PREFIX": prefix,
        "GITHUB_API_URL": "https://example.invalid",
        "GITHUB_REPOSITORY": "fixture/repo", "GITHUB_OUTPUT": str(output),
    }.items():
        monkeypatch.setenv(key, value)

    def response(request, timeout):
        assert request.full_url.endswith("/git/ref/tags/" + prefix + "1.0.6+sha256.0123456789abcdef")
        if status == 200:
            return io.BytesIO(b"")
        raise urllib.error.HTTPError(request.full_url, status, "fixture", {}, None)

    monkeypatch.setattr("urllib.request.urlopen", response)
    if expected is None:
        with pytest.raises(SystemExit, match="tag lookup failed"):
            exec(compile(code, "<workflow-tag-check>", "exec"), {})
        assert not output.exists()
    else:
        exec(compile(code, "<workflow-tag-check>", "exec"), {})
        assert output.read_text() == "exists=" + expected + "\n"


@pytest.mark.parametrize("branch,prefix,title,flags", [
    ("main", "v", "Cortex Workspace", ["--latest"]),
    ("dev", "dev-v", "Cortex Workspace Dev", ["--prerelease", "--latest=false"]),
])
def test_publication_channel_arguments(tmp_path, branch, prefix, title, flags):
    import os
    step = workflow()["jobs"]["publish"]["steps"][-1]
    # Shell function substitutes only the outward-facing command; no GitHub call.
    script = 'gh() { printf "%s\\n" "$@" > "$CAPTURE"; }\n' + step["run"]
    capture = tmp_path / "arguments"
    version = "1.0.6+z" + STAMP + ".sha256.0123456789abcdef"
    subprocess.run(["bash", "-e", "-c", script], check=True, env={
        **os.environ, "GITHUB_REF": "refs/heads/" + branch, "TAG_PREFIX": prefix,
        "VERSION": version, "GITHUB_SHA": "a" * 40, "CAPTURE": str(capture),
    })
    arguments = capture.read_text().splitlines()
    assert arguments[:3] == ["release", "create", prefix + version]
    assert arguments[arguments.index("--title") + 1] == title + " " + version
    assert arguments[-len(flags):] == flags
    if branch == "dev":
        assert "Development build" in arguments[arguments.index("--notes") + 1]


@pytest.mark.parametrize("helper", ["maintenance.py", "engine_connections.py", "owner_workflows.py"])
def test_runtime_host_changes_invalidate_content_identity(tmp_path, helper):
    import shutil
    package = load("scripts/cortex_package.py")
    root = tmp_path / "payload"
    directory = root / "crates/workspace-service"
    directory.mkdir(parents=True)
    target = directory / helper
    shutil.copyfile(ROOT / "crates/workspace-service" / helper, target)
    before = package.payload_digest(root)
    target.write_bytes(target.read_bytes() + b"\n# changed runtime helper\n")
    assert package.payload_digest(root) != before


def test_native_window_embeds_the_cortex_logo():
    config = json.loads((ROOT / "apps/cortex-workspace-desktop/src-tauri/tauri.conf.json").read_text())
    assert config["bundle"]["icon"] == ["icons/icon.png"]
    icon = ROOT / "apps/cortex-workspace-desktop/src-tauri/icons/icon.png"
    assert icon.read_bytes() == (ROOT / "assets/logo.png").read_bytes()


def engine_fixture(tmp_path):
    """Small byte inventory; the no-network container check uses the actual npm tree."""
    import hashlib
    tree = tmp_path / "engines"
    for relative in ("node/bin/node", "node/LICENSE",
                     "node_modules/@agentclientprotocol/codex-acp/dist/index.js",
                     "node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
                     "node_modules/dependency/index.js"):
        path = tree / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture")
    (tree / "node/bin/node").chmod(0o755)
    (tree / "package-lock.json").write_bytes((ROOT / "packaging/engines/package-lock.json").read_bytes())
    files = {str(p.relative_to(tree)): hashlib.sha256(p.read_bytes()).hexdigest()
             for p in tree.rglob("*") if p.is_file()}
    (tree / "bundle-manifest.json").write_text(json.dumps(dict(
        files=files, required=list(files), lock_sha256=files["package-lock.json"])))
    return tree


def voice_fixture(base):
    import hashlib
    tree = base / "voice"
    for name in ("voice.cjs", "model.json", "node_modules/sherpa-onnx-node/sherpa-onnx.js",
                 "node_modules/sherpa-onnx-linux-x64/sherpa-onnx.node",
                 "node_modules/sherpa-onnx-linux-x64/libonnxruntime.so",
                 "node_modules/sherpa-onnx-linux-x64/libsherpa-onnx-c-api.so"):
        file = tree / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text("voice fixture")
    for name in ("voice.cjs", "model.json", "package.json", "package-lock.json"):
        (tree / name).write_bytes((ROOT / "packaging/voice" / name).read_bytes())
    files = {str(p.relative_to(tree)): hashlib.sha256(p.read_bytes()).hexdigest()
             for p in tree.rglob("*") if p.is_file()}
    (tree / "bundle-manifest.json").write_text(json.dumps(dict(files=files, lock_sha256=files["package-lock.json"])))
    return tree


@pytest.mark.parametrize("authorization", ["github-main-clean", "local-owner-test"])
def test_packages_include_complete_runtime_and_automatic_user_units(tmp_path, monkeypatch, authorization):
    monkeypatch.setenv("CORTEX_WORKSPACE_RELEASE_AUTHORIZED", authorization)
    module = load("packaging/container/package_channel.py")
    target = tmp_path / "target/release"
    target.mkdir(parents=True)
    for binary in ("cortex-workspace-desktop", "workspace-service"):
        (target / binary).write_text("fixture executable")
    engines = engine_fixture(tmp_path)
    voice = voice_fixture(tmp_path)
    layouts = {}
    for name in ("dev", "release"):
        output = tmp_path / name
        module.package(ROOT, target.parent, output, name, "libc6 (>= 2.42)", engines=engines, stamp=STAMP)
        archive = next(output.glob("*.deb"))
        listing = subprocess.check_output(["dpkg-deb", "-c", str(archive)], text=True)
        assert listing.splitlines()[0].startswith("drwxr-xr-x root/root")
        control = subprocess.check_output(["dpkg-deb", "-f", str(archive)], text=True)
        assert "python3 (>= 3.11)" in control and "libc6 (>= 2.42)" in control
        assert "-dev" not in next(line for line in control.splitlines() if line.startswith("Depends:"))
        identity = json.loads((ROOT / "package-identity.json").read_text())
        version = module.package_version(identity["version"], STAMP, name)
        assert "Version: " + version + "\n" in control
        assert version.endswith(".local") is (name == "release" and authorization == "local-owner-test")
        assert re.fullmatch(r"1\.0\.6\+z[0-9]{20}\.sha256\.[0-9a-f]{16}(?:\.local)?", version)
        assert version in archive.name
        stage = tmp_path / ("unpacked-" + name)
        subprocess.run(["dpkg-deb", "-R", str(archive), str(stage)], check=True)
        assert {p.name for p in (stage / "DEBIAN").iterdir()} == {"control", "postinst", "prerm"}
        for script in ("postinst", "prerm"):
            subprocess.run(["sh", "-n", str(stage / "DEBIAN" / script)], check=True)
        assert "try-restart" in (stage / "DEBIAN/postinst").read_text()
        assert "stop" in (stage / "DEBIAN/prerm").read_text()
        c = module.channel(ROOT, name)
        data = stage / "usr/share" / c["package"]
        assert (data / "schema.json").read_bytes() == (ROOT / "src/cortex_runtime/workspace_contract/schema.json").read_bytes()
        assert json.loads((data / "package-identity.json").read_text()) == identity
        assert "SIL OPEN FONT LICENSE" in (stage / "usr/share/doc" / c["package"] / "fonts.html").read_text()
        entry = configparser.ConfigParser(interpolation=None)
        entry.optionxform = str
        entry.read(stage / "usr/share/applications" / (c["package"] + ".desktop"))
        launcher = entry["Desktop Entry"]
        assert launcher["Icon"] == c["package"]
        assert launcher["StartupWMClass"] == c["identifier"]
        for size in module.ICON_SIZES:
            icon = stage / "usr/share/icons/hicolor" / f"{size}x{size}" / "apps" / (c["package"] + ".png")
            assert icon.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"
            assert int.from_bytes(icon.read_bytes()[16:20], "big") == size
        assert (stage / "usr/share/icons/hicolor/512x512/apps" / (c["package"] + ".png")).read_bytes() == (ROOT / "assets/logo.png").read_bytes()
        unit = configparser.ConfigParser(interpolation=None)
        unit.read(stage / "usr/lib/systemd/user" / c["unit"])
        service = unit["Service"]
        assert service["Type"] == "simple"
        assert service["Restart"] == "on-failure" and service["RestartSec"] == "5s"
        assert service["KillMode"] == "control-group" and service["TimeoutStopSec"] == "30s"
        assert unit["Unit"]["StartLimitBurst"] == "5"
        assert "EnvironmentFile" not in service
        assert "${" not in service["ExecStart"]
        assert "/usr/bin/python3 /usr/lib/" + c["package"] + "/runtime" in service["ExecStart"]
        # Projects live anywhere the owner can write, /mnt included. The home is not a project boundary.
        assert service["ExecStart"].split()[1] == "/"
        wants = stage / "usr/lib/systemd/user/default.target.wants" / c["unit"]
        assert wants.is_symlink() and wants.readlink() == Path("../" + c["unit"])
        runtime = stage / "usr/lib" / c["package"] / "runtime"
        for relative, source in module.payload_files(ROOT):
            assert (runtime / relative).read_bytes() == source.read_bytes(), relative
            assert (runtime / relative).stat().st_mode & 0o444 == 0o444, relative
        for required in ("skills", "policy", "report-templates", "agent-sources"):
            assert (runtime / required).is_dir()
        assert all(not any(part in {".git", "tests", "prototypes", "node_modules"} for part in p.relative_to(runtime).parts) for p in runtime.rglob("*"))
        assert all(dependency in control for dependency in ("bubblewrap", "dbus-user-session", "pulseaudio-utils", "bzip2"))
        for relative in json.loads((voice / "bundle-manifest.json").read_text())["files"]:
            assert (runtime.parent / "voice" / relative).read_bytes() == (voice / relative).read_bytes()
        assert not re.search(r"\\b(nodejs|npm)\\b", control)
        bundled = runtime.parent / "engines"
        for relative in json.loads((engines / "bundle-manifest.json").read_text())["files"]:
            assert (bundled / relative).read_bytes() == (engines / relative).read_bytes()
        assert c["service_binary"] in service["ExecStart"]
        assert "systemctl" not in service["ExecStart"]
        layouts[name] = {str(p.relative_to(stage)) for p in (stage / "usr").rglob("*") if p.is_file()}
    assert not layouts["dev"] & layouts["release"]
    again = tmp_path / "repeat"
    module.package(ROOT, target.parent, again, "dev", "libc6 (>= 2.42)", engines=engines, stamp=STAMP)
    assert next(again.glob("*.deb")).read_bytes() == next((tmp_path / "dev").glob("*.deb")).read_bytes()


@pytest.mark.parametrize("dependency", ["libgtk-3-dev", "nodejs (>= 20)", "npm"])
def test_package_rejects_development_dependencies(tmp_path, dependency):
    module = load("packaging/container/package_channel.py")
    with pytest.raises(ValueError, match="runtime dependencies"):
        module.package(ROOT, tmp_path, tmp_path / "output", "dev", dependency, stamp=STAMP)
    recipe = (ROOT / "packaging/container/build.sh").read_text()
    assert "dpkg-shlibdeps -O" in recipe and "--depends-file /build/shlibs.depends" in recipe


def test_missing_required_runtime_asset_fails_packaging(tmp_path):
    module = load("packaging/container/package_channel.py")
    # A damaged source snapshot must fail before it can become an archive.
    import shutil
    module_root = tmp_path / "snapshot"
    builder = load("scripts/build_workspace_desktop.py")
    builder.snapshot(ROOT, module_root)
    (module_root / "policy/model-settings-default.json").unlink()
    with pytest.raises(ValueError, match="runtime content stamp mismatch"):
        module.payload_files(module_root)
    # The asset set is also checked byte-for-byte in the archive above. Missing
    # top-level roots and declared Python modules fail enumeration immediately.
    shutil.rmtree(module_root / "report-templates")
    with pytest.raises(ValueError, match="missing runtime payload entry"):
        module.payload_files(module_root)


@pytest.mark.parametrize("missing", [
    "node/bin/node",
    "node_modules/@agentclientprotocol/codex-acp/dist/index.js",
    "node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
    "node_modules/dependency/index.js",
])
def test_incomplete_engine_tree_fails_packaging(tmp_path, missing):
    module = load("packaging/container/package_channel.py")
    engines = engine_fixture(tmp_path)
    (engines / missing).unlink()
    with pytest.raises(ValueError, match="incomplete engine bundle"):
        module.package(ROOT, tmp_path, tmp_path / "output", "dev", engines=engines, stamp=STAMP)


def test_engine_lock_matches_pins_and_integrities(tmp_path):
    module = load("packaging/container/bundle_engines.py")
    module.validate_lock(ROOT)
    import shutil
    shutil.copytree(ROOT / "packaging/engines", tmp_path / "packaging/engines")
    shutil.copyfile(ROOT / "pinned-dependencies.json", tmp_path / "pinned-dependencies.json")
    pins = json.loads((tmp_path / "pinned-dependencies.json").read_text())
    pins["engines"]["codex-acp"]["pinned_package_version"] = "999.0.0"
    (tmp_path / "pinned-dependencies.json").write_text(json.dumps(pins))
    with pytest.raises(ValueError, match="differs from pinned"):
        module.validate_lock(tmp_path)


def test_incomplete_voice_bundle_is_refused(tmp_path):
    module = load("packaging/container/bundle_voice.py")
    tree = voice_fixture(tmp_path)
    module.validate_tree(tree, ROOT / "packaging/voice")
    (tree / "voice.cjs").write_text("changed")
    with pytest.raises(ValueError, match="incomplete voice bundle"):
        module.validate_tree(tree, ROOT / "packaging/voice")


@pytest.mark.parametrize("old_hash", ["0000000000000000", "f3eee9da173725e1", "ffffffffffffffff"])
@pytest.mark.parametrize("new_hash", ["0000000000000000", "f000000000000000", "ffffffffffffffff"])
def test_debian_versions_sort_after_every_legacy_hash(old_hash, new_hash):
    version = load("packaging/container/debian_version.py").debian_version
    newer = version("1.0.6+sha256." + new_hash, STAMP)
    subprocess.run(["dpkg", "--compare-versions", newer, "gt", "1.0.6+sha256." + old_hash], check=True)
    # Time, not hash magnitude, must decide the ordering for subsequent builds.
    following = version("1.0.6+sha256.0000000000000000", "20261106060000000001")
    subprocess.run(["dpkg", "--compare-versions", following, "gt", newer], check=True)


@pytest.mark.parametrize("identity", ["1.0.6", "1.0.6+sha256.FFFFFFFFFFFFFFFF",
                                    "1x0x6+sha256.ffffffffffffffff", "1.0.6+sha256.abc\n"])
def test_debian_version_rejects_invalid_identity(identity):
    with pytest.raises(ValueError, match="package version"):
        load("packaging/container/debian_version.py").debian_version(identity, STAMP)


def test_python_payload_is_complete():
    declared = json.loads((ROOT / "runtime-payload.json").read_text())["files"]
    actual = sorted(str(p.relative_to(ROOT)) for p in (ROOT / "src").rglob("*.py"))
    assert actual == declared
    assert "src/cortex_runtime/workspace_contract/__init__.py" in declared
    assert "src/cortex_runtime/workspace_api/__init__.py" in declared
