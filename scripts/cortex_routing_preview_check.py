#!/usr/bin/env python3
"""Assert a nonmutating, routing-only candidate preview; never read registries."""
from __future__ import annotations

import argparse
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))
from cortex_runtime.configuration import management as m, model_policy_migration as migration


def read_regular(path):
    info = path.lstat()
    assert stat.S_ISREG(info.st_mode) and info.st_nlink == 1
    assert info.st_uid == os.getuid() and not info.st_mode & 0o077
    assert info.st_size <= 262144
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        opened = os.fstat(descriptor)
        assert (opened.st_dev, opened.st_ino) == (info.st_dev, info.st_ino)
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            data = stream.read(262145)
        assert len(data) <= 262144
        return data
    finally:
        os.close(descriptor)


def complete_missing(current, additions):
    """Retain every existing value/order; complete only absent reviewed fields."""
    result = deepcopy(current)
    for key, value in additions.items():
        if key not in result:
            result[key] = deepcopy(value)
        elif isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = complete_missing(result[key], value)
    return result


def check(config_root, template):
    root = Path(config_root)
    assert root.is_absolute() and root.name == "cortex"
    for directory in (root, *root.parents):
        assert stat.S_ISDIR(directory.lstat().st_mode)
    assert root.stat().st_uid == os.getuid() and not root.stat().st_mode & 0o077
    paths = [root / name for name in ("routing.json", "defaults.json")]
    before = [read_regular(path) for path in paths]
    source, document = (json.loads(body) for body in before)
    # Never copy arbitrary private settings into a candidate or public artifact.
    assert isinstance(source, dict) and set(source) <= set(m.ROUTING_KEYS)
    assert isinstance(document, dict) and isinstance(document.get("defaults"), dict)
    values = document["defaults"]
    assert isinstance(values.get("parameters", {}), dict)
    assert all(isinstance(k, str) and isinstance(v, str) for k, v in values.get("parameters", {}).items())
    if "coordinator" in values:
        from cortex_runtime.configuration.resolution import validate_portable_coordinator
        validate_portable_coordinator(values["coordinator"])
    else:
        assert isinstance(values.get("route_model"), str) and values["route_model"]
    reviewed = json.loads(Path(template).read_text())["routing"]
    m.validate_routing_rules(reviewed)
    candidate = complete_missing(source, reviewed)
    candidate["model_policy_version"] = 2
    m.validate_routing_rules(candidate)
    migration._preserve(source, candidate)
    candidate_body = m._pretty(candidate)
    environ = dict(os.environ, XDG_CONFIG_HOME=str(root.parent), PYTHONDONTWRITEBYTECODE="1")
    # CLI selects only the explicit user root, never project/profile/registry data.
    with tempfile.TemporaryDirectory(prefix="cortex-routing-preview-") as directory:
        path = Path(directory) / "candidate.json"
        path.write_bytes(candidate_body)
        path.chmod(0o600)
        api = migration.preview(path, environ=environ)
        command = [sys.executable, "-B", str(ROOT / "scripts/cortex_model_policy_migrate.py"),
                   "preview", "--candidate", str(path)]
        process = subprocess.run(command, env=environ, capture_output=True, text=True, timeout=30)
        assert process.returncode == 0 and not process.stderr
        cli = json.loads(process.stdout)
        assert cli["ok"] and cli["result"] == api
        assert api["applied"] is False and api["profile"] is None
        assert api["expected_revision"] == "sha256:" + hashlib.sha256(before[0]).hexdigest()
        assert api["candidate_sha256"] == hashlib.sha256(candidate_body).hexdigest()
        assert api["proposed_revision"] == "sha256:" + hashlib.sha256(candidate_body).hexdigest()
        assert path.read_bytes() == candidate_body
    after = [read_regular(path) for path in paths]
    assert after == before
    # No private identifiers, setting bodies, profile names or host output.
    return dict(assertions_passed=True, cli_preview_exit=0, preview=api,
                defaults_sha256=hashlib.sha256(before[1]).hexdigest(),
                unchanged_routing=True, unchanged_defaults=True, candidate_retained=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config-root", required=True)
    parser.add_argument("--template", default=str(ROOT / "policy/model-settings-default.json"))
    args = parser.parse_args()
    try:
        result = check(args.config_root, args.template)
    except Exception:
        print(json.dumps(dict(assertions_passed=False, diagnostic="preview_assertion_failed")))
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    from _cortex_tmp import prefer_disk_tmp
    prefer_disk_tmp()  # /var/tmp, not a RAM-backed /tmp
    raise SystemExit(main())
