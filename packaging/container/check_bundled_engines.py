#!/usr/bin/env python3
"""Offline install + real Cortex launch/ACP initialize. Never sends a prompt or logs in."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

BASE = "ubuntu:26.04@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c"


def session(package):
    runtime = Path("/usr/lib") / package / "runtime"
    tree = runtime.parent / "engines"
    sys.path.insert(0, str(runtime / "src"))
    from cortex_runtime.connections import automatic, discovery, onboarding, engine_update, registry
    from cortex_runtime.engine.install_resolution import resolve_verified_install
    from cortex_runtime.engine.instruction_delivery_registry import has_exact_registration
    from cortex_runtime.engine.process import EngineProcess
    from cortex_runtime.engine.client import EngineClient
    from cortex_runtime.engine.correlation import CorrelationTables

    assert not shutil.which("node") and not shutil.which("npm")
    manifest = json.loads((tree / "bundle-manifest.json").read_text())
    before = {}
    for relative, expected in manifest["files"].items():
        path = tree / relative
        assert path.stat().st_uid == 0 and not os.access(path, os.W_OK)
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        assert actual == expected
        before[relative] = actual
    env = dict(os.environ, CORTEX_ENGINE_UPDATE="off")
    workspace = Path.home() / "project"
    workspace.mkdir()
    for subdir in (".codex", ".claude"):
        (Path.home() / subdir).mkdir(mode=0o700)
    for engine, provider in (("codex-acp", "openai"), ("claude-agent-acp", "anthropic")):
        found = discovery.discover_engine(engine, environ=env)
        assert found and discovery.is_qualified(engine, found.version)
        assert has_exact_registration(engine, found.version)
        assert Path(found.launch_command[0]) == tree / "node/bin/node"
        assert not engine_update.update_engine(engine, environ=env)
        # No login marker must not become a ready connection just because the adapter exists.
        automatic._setup_engine(engine, env)
        assert automatic.read_status(engine, env) == "login_required"
        assert not any(r.engine == engine for r in registry.read_registry(env).connections.values())
        marker = Path.home() / (".codex/auth.json" if engine == "codex-acp" else ".claude/.credentials.json")
        marker.write_text("{}")
        marker.chmod(0o600)
        created = onboarding.sync_discovered_connections({engine: found}, environ=env)
        assert len(created) == 1
        record = created[0]
        assert not record.credential_ref.owned and record.state.value == "configured"
        print(json.dumps(dict(engine=engine, missing_marker="login_required", synthetic_marker="configured",
                              real_account_authentication="unverified", model_refresh="not_run")), flush=True)
        install = resolve_verified_install(record, env)
        process = EngineProcess(record, str(workspace), environ=env, install=install)
        client = EngineClient(process, correlation=CorrelationTables(), on_update=lambda _: None,
                              on_client_request=lambda *_: None).start()
        try:
            result = client.call("initialize", dict(protocolVersion=1, clientCapabilities={},
                         clientInfo=dict(name="cortex-offline-install-check", version="1.0.6")), timeout=20)
            assert result["protocolVersion"] == 1
            print(json.dumps(dict(engine=engine, version=found.version, initialize="passed",
                                  prompts_sent=0, bundled_node=True)), flush=True)
        finally:
            process.release()
            client.close()
        if engine == "codex-acp":
            native = Path(install.bundled_codex_binary)
        else:
            arch = manifest["architecture"]
            native = tree / ("node_modules/@anthropic-ai/claude-agent-sdk-linux-" + arch) / "claude"
        subprocess.run([str(native), "--version"], check=True, timeout=20,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    assert all(hashlib.sha256((tree / p).read_bytes()).hexdigest() == h for p, h in before.items())
    print(json.dumps(dict(root_owned_bundle_unchanged=True, network="none", native_version_checks="passed")), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--session", choices=("cortex-workspace-dev", "cortex-workspace"))
    args = parser.parse_args()
    if args.session:
        session(args.session)
        return
    archive = args.archive.resolve(strict=True)
    package = subprocess.check_output(["dpkg-deb", "-f", str(archive), "Package"], text=True).strip()
    assert package in {"cortex-workspace-dev", "cortex-workspace"}
    depends = subprocess.check_output(["dpkg-deb", "-f", str(archive), "Depends"], text=True)
    assert not re.search(r"\b(nodejs|npm)\b", depends)
    names = [re.match(r"[a-z0-9.+-]+", part.strip()).group() for part in depends.split(",")]
    with tempfile.TemporaryDirectory(prefix="cortex-offline-install-") as temporary:
        context = Path(temporary)
        shutil.copyfile(archive, context / "application.deb")
        shutil.copyfile(__file__, context / "check.py")
        (context / "Dockerfile").write_text(
            "FROM " + BASE + "\nRUN apt-get update && apt-get install -y --no-install-recommends "
            + " ".join(names) + " && rm -rf /var/lib/apt/lists/*\n"
            + "COPY application.deb /application.deb\nCOPY check.py /check.py\n")
        tag = "cortex-offline-engines:" + hashlib.sha256(archive.read_bytes()).hexdigest()[:16]
        subprocess.run(["docker", "build", "-q", "-t", tag, str(context)], check=True, timeout=600)
        subprocess.run(["docker", "run", "--rm", "--network", "none", "--entrypoint", "sh", tag, "-c",
                        "dpkg -i /application.deb >/dev/null && useradd --create-home smoke && "
                        "runuser -u smoke -- env HOME=/home/smoke PATH=/usr/bin:/bin "
                        "python3 -B /check.py --session " + package], check=True, timeout=90)
    print(json.dumps(dict(archive_bytes=archive.stat().st_size, package=package)), flush=True)


if __name__ == "__main__":
    main()
