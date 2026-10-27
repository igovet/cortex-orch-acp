"""Development regressions. Unjailed receipt tests do NOT qualify containment/Docker."""
import os
import sys
import threading
import time
from pathlib import Path

import pytest

from cortex_runtime.acp import sessions
from cortex_runtime.configuration import access_mode
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.evidence import command_identity
from cortex_runtime.engine.permissions import declared_command_deletion
from cortex_runtime.orchestration import gateway, sandbox
from cortex_runtime.trusted_channel import resolve_channel
from test_access_mode import _store_and_task
from test_command_run_arguments import _worker


@pytest.fixture
def development(tmp_path, monkeypatch):
    root, store, channel, secret = _worker(tmp_path)
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    for key in ("CODEX_HOME", "CLAUDE_CONFIG_DIR"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("XDG_CONFIG_HOME", str(home / ".config"))
    monkeypatch.setenv("XDG_STATE_HOME", str(home / ".local/state"))
    access_mode.switch(store, resolve_channel(store, channel, secret).task_id,
                       "development", origin="acp_set_mode")
    return root, store, resolve_channel(store, channel, secret), home


def test_durable_default_legacy_and_idempotent_binding(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    project = store.project_root
    sessions.create_session(store, "fresh", project, {}, {}, [])
    assert sessions.session_access_mode(store, "fresh") == "development"
    sessions.bind_task(store, "fresh", task_id)
    sessions.bind_task(store, "fresh", task_id)
    assert access_mode.current(store, task_id) == "development"
    assert len(access_mode.history(store, task_id)) == 1
    access_mode.switch(store, task_id, "guarded", origin="acp_set_mode")
    sessions.bind_task(store, "fresh", task_id)
    assert access_mode.current(store, task_id) == "guarded"
    # Simulate an old row with no additive initial record; reads never backfill.
    sessions.create_session(store, "legacy", project, {}, {}, [])
    with store.connection() as db:
        db.execute("DELETE FROM acp_session_access_defaults WHERE session_id='legacy'")
        db.commit()
    assert sessions.session_access_mode(store, "legacy") == "guarded"
    assert sessions.session_access_mode(store, "unknown") == "guarded"


@pytest.mark.parametrize("mode", ["guarded", "auto"])
def test_explicit_session_choice_survives_reopen_and_wins(tmp_path, mode):
    store, task_id = _store_and_task(tmp_path)
    sessions.create_session(store, "fresh", store.project_root, {}, {}, [])
    sessions.select_session_access(store, "fresh", mode, "acp_set_mode")
    assert sessions.session_access_mode(store, "fresh") == mode
    sessions.bind_task(store, "fresh", task_id)
    assert access_mode.current(store, task_id) == mode
    assert access_mode.history(store, task_id)[0]["origin"] == "acp_pre_task_selection"
    with pytest.raises(access_mode.InvalidAccessMode):
        sessions.select_session_access(store, "fresh", "development", "model")


@pytest.mark.parametrize("choice,expected", [
    (None, "development"), ("guarded", "guarded"), ("auto", "auto"), ("legacy", "guarded"),
])
def test_install_snapshot_does_not_override_durable_session_access(tmp_path, choice, expected):
    import json
    from cortex_runtime.connections import registry
    from cortex_runtime.routing import defaults
    from cortex_runtime.store import Store

    home = tmp_path / "home"
    home.mkdir()
    env = dict(HOME=str(home), XDG_CONFIG_HOME=str(tmp_path / "config"),
               XDG_STATE_HOME=str(tmp_path / "state"))
    result = defaults.apply(env)
    assert result["defaults"]["status"] == "created"
    installed = registry.config_root(env) / "defaults.json"
    owner_bytes = installed.read_bytes()
    assert json.loads(owner_bytes)["defaults"]["coordinator"] == {
        "engine": "claude-agent-acp", "model": "opus[1m]", "parameters": {"thought_level": "medium"}}

    store, task_id = _store_and_task(tmp_path)
    sessions.create_session(store, "installed", store.project_root, {}, {}, [])
    if choice == "legacy":
        with store.connection() as db:
            db.execute("DELETE FROM acp_session_access_defaults WHERE session_id='installed'")
            db.commit()
    elif choice is not None:
        sessions.select_session_access(store, "installed", choice, "acp_set_mode")
    sessions.bind_task(store, "installed", task_id)

    # Reinstall and reopen must not turn a routing preference into access authority.
    assert defaults.apply(env)["defaults"]["changed"] is False
    assert installed.read_bytes() == owner_bytes
    reopened = Store(Path(store.project_root) / ".cortex", project_root=store.project_root)
    sessions.bind_task(reopened, "installed", task_id)
    assert sessions.session_access_mode(reopened, "installed") == expected
    assert access_mode.current(reopened, task_id) == expected
    assert len(access_mode.history(reopened, task_id)) == (0 if choice == "legacy" else 1)


def test_real_launch_preparation_has_host_home_and_masks(development, tmp_path):
    root, store, worker, home = development
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    (home / ".codex").mkdir()
    environ = dict(HOME=str(home), PATH="/usr/bin:/bin", DOCKER_HOST="unix:///run/docker.sock",
                   CORTEX_CHANNEL_SECRET="fixture", PRIVATE_TOKEN="fixture")
    with sandbox.development_command(root, scratch, ["true"], ["."], environ=environ) as (argv, fds):
        assert argv[argv.index("--bind"):][:3] == ["--bind", "/", "/"]
        assert "/var/tmp" not in argv
        assert ["--ro-bind", str(home), str(home)] not in [argv[i:i+3] for i in range(len(argv))]
        assert ["--ro-bind", str(home / ".codex"), str(home / ".codex")] in [argv[i:i+3] for i in range(len(argv))]
        assert "--clearenv" in argv and fds
        assert "DOCKER_HOST" in argv and "PRIVATE_TOKEN" not in argv and "CORTEX_CHANNEL_SECRET" not in argv


@pytest.mark.usefixtures("unjailed_gateway_command")
@pytest.mark.parametrize("mode", ["development", "auto"])
def test_receipt_outside_cwd_and_filtered_environment(development, mode):
    root, store, worker, home = development
    access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
    receipt = gateway.dispatch(store, worker, "command_run", dict(
        argv=[sys.executable, "-B", "-c", "import os;print(os.getcwd());assert not any(k.startswith('CORTEX_') for k in os.environ)"],
        cwd=str(home), request_key="outside"))
    assert receipt["exit_code"] == 0 and receipt["complete"], receipt
    assert receipt["cwd"] == str(home) and receipt["profile"] == "development"
    assert receipt["output"].strip() == str(home)
    access_mode.switch(store, worker.task_id, "guarded", origin="acp_set_mode")
    with pytest.raises(StoreError, match="gateway_command_cwd_denied"):
        gateway.dispatch(store, worker, "command_run", dict(argv=["true"], cwd=str(home), request_key="guarded"))


@pytest.mark.parametrize("argv", [
    ["rm", "file"], ["/bin/rm", "file"], ["sudo", "rm", "file"],
    ["sudo", "-n", "-u", "fixture", "--", "/bin/rm", "file"],
    ["xargs", "rm"], ["xargs", "-n", "1", "--", "rm"],
    ["exec", "-a", "fixture", "rm", "file"],
    ["env", "rm", "file"], ["/usr/bin/env", "-i", "LABEL=fixture", "/bin/rm", "file"],
    ["env", "-u", "LABEL", "--", "rm", "file"],
    ["env", "--", "LABEL=fixture", "rm", "file"],
    ["xargs", "--replace", "rm", "{}"],
    ["sh", "-c", "echo before; rm file"], ["sh", "-c", " rm file"],
    ["bash", "-lc", "\t rm file"], ["dash", "-c", "\n  /bin/rm file"],
    ["zsh", "-c", "  rm file"], ["env", "sh", "-c", "rm file"],
    ["env", "bash", "-lc", "rm file"], ["env", "dash", "-c", " rm file"],
    ["env", "zsh", "-c", "\t rm file"],
    ["env", "LABEL=fixture", "sudo", "-n", "dash", "-c", " rm file"],
    ["bash", "-o", "pipefail", "-c", "rm file"],
    ["bash", "-eo", "pipefail", "-c", "env rm file"],
    ["sh", "-c", "if true; then sudo rm file; fi"],
    ["sh", "-c", "LABEL=fixture rm fixture"],
    ["sh", "-c", "if true; then env rm fixture; fi"],
    ["sh", "-c", "LABEL=fixture OTHER='two words' env -i /bin/rm fixture"],
    ["sh", "-c", "LABEL='text; env rm argument' env rm fixture"],
    ["sh", "-c", "echo before && LABEL=fixture env rm fixture"],
    ["sh", "-c", "echo before\nLABEL=fixture env rm fixture"],
    ["sh", "-c", "if LABEL=fixture env rm fixture; then echo done; fi"],
    ["sh", "-c", "if false; then echo safe; elif env rm fixture; then echo done; fi"],
    ["sh", "-c", "if false; then echo safe; else LABEL=fixture env rm fixture; fi"],
    ["sh", "-c", "while env rm fixture; do echo done; done"],
    ["sh", "-c", "until LABEL=fixture env rm fixture; do echo done; done"],
    ["sh", "-c", "for item in fixture; do LABEL=fixture env rm fixture; done"],
    ["sh", "-c", "while true; do env rm fixture; done"],
    ["sh", "-c", "! LABEL=fixture env rm fixture"],
    ["sh", "-c", "if ! env rm fixture; then echo done; fi"],
    ["sh", "-c", "{ LABEL=fixture env rm fixture; }"],
    ["sh", "-c", "(LABEL=fixture env rm fixture)"],
    ["sh", "-c", "if (env rm fixture); then echo done; fi"],
    ["sh", "-c", "LABEL=fixture dash -c 'if true; then env rm fixture; fi'"],
    ["sh", "-c", "if true; then # env echo comment\n LABEL=fixture env rm fixture; fi"],
    ["bash", "+o", "pipefail", "-lc", "env rm file"],
    ["bash", "-O", "extglob", "-c", "env rm file"],
    ["bash", "--rcfile", "fixture", "-c", "rm file"],
    ["sh", "-o", "errexit", "-c", "env rm file"],
    ["sh", "-c", "env rm file"],
    ["sh", "-c", "/usr/bin/env -u LABEL -- rm file"],
    ["sh", "-c", "echo before; env LABEL=fixture /bin/rm file"],
    ["sh", "-c", "echo before && env rm file"],
    ["sh", "-c", "echo before | env rm file"],
    ["sh", "-c", "echo before\nenv rm file"],
    ["sh", "-c", "env dash -c 'rm file'"],
    ["sh", "-c", "env -i LABEL=fixture dash -c 'env rm file'"],
    ["env", "dash", "-o", "errexit", "-c", "env rm file"],
    ["sh", "-c", "env bash -o pipefail -c 'env rm file'"],
    ["find", ".", "-exec", "rm", "{}", ";"],
    ["find", ".", "-execdir", "env", "rm", "{}", "+"],
])
@pytest.mark.parametrize("read_only", [False, True])
def test_declared_deletion_requires_consent_before_launch(development, monkeypatch, argv, read_only):
    root, store, worker, home = development
    called = []
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, "request", lambda *a, **kw: called.append(a[2]) or False)
    def unexpected_effect(*args, **kwargs):
        pytest.fail("deletion refusal must precede cwd, mounts and launch")
    monkeypatch.setattr(gateway, "_command_cwd", unexpected_effect)
    monkeypatch.setattr(sandbox, "development_command", unexpected_effect)
    monkeypatch.setattr(sandbox, "gateway_command", unexpected_effect)
    monkeypatch.setattr(gateway.subprocess, "Popen", unexpected_effect)
    assert declared_command_deletion(argv)
    if read_only:
        monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    with pytest.raises(StoreError, match="gateway_scope_denied" if read_only else "deletion_not_confirmed"):
        gateway.dispatch(store, worker, "command_run", dict(argv=argv, request_key="delete"))
    assert called == ([] if read_only else ["command_delete"])


@pytest.mark.parametrize("argv", [
    [], ["echo", "rm file"], ["echo", "text; rm file"],
    ["printf", "%s", "text | rm file\nrm file"],
    ["echo", "sudo rm file"], ["echo", "find . -exec rm {} ;"],
    ["printf", "%s", "sh -c 'rm file'"],
    ["env", "LABEL=rm file", "echo", "text; rm file"],
    ["sudo", "-u", "rm", "echo", "text; rm file"],
    ["xargs", "-I", "rm", "echo", "text; rm file"],
    ["exec", "-a", "rm", "echo", "text; rm file"],
    ["sh", "script.sh", "-c", "rm file"],
    ["sh", "--", "-c", "rm file"],
    ["bash", "-o", "pipefail", "script.sh", "-c", "rm file"],
    ["bash", "--rcfile", "-c", "rm file"],
    ["bash", "-o", "pipefail", "-c"],
    ["bash", "-o"],
    ["sh", "-c", "env LABEL=rm echo safe"],
    ["LABEL=fixture", "rm", "fixture"],
    ["then", "env", "rm", "fixture"],
    ["sh", "-c", "LABEL=fixture OTHER='two words' echo env rm fixture"],
    ["sh", "-c", "LABEL='env rm fixture'"],
    ["sh", "-c", "'LABEL=fixture' env rm fixture"],
    ["sh", "-c", "'then' env rm fixture"],
    ["sh", "-c", "\\then env rm fixture"],
    ["sh", "-c", "if true; then printf '%s' 'env rm fixture'; fi"],
    ["sh", "-c", "if true; then echo 'LABEL=fixture env rm fixture'; fi"],
    ["sh", "-c", "LABEL=fixture echo then env rm fixture"],
    ["sh", "-c", "LABEL=fixture then env rm fixture"],
    ["sh", "-c", "for rm in fixture; do echo safe; done"],
    ["sh", "-c", "case rm in fixture) echo safe;; esac"],
    ["sh", "-c", "printf '%s' '(LABEL=fixture env rm fixture)'"],
    ["sh", "-c", "echo \\( env rm fixture \\)"],
    ["sh", "-c", "if true; then # LABEL=fixture env rm fixture\n echo safe; fi"],
    ["sh", "-c", "env -u rm echo safe"],
    ["sh", "-c", "echo 'env rm file'"],
    ["sh", "-c", "printf '%s' ';' env rm file"],
    ["sh", "-c", "echo before # comment; env rm file"],
    ["sh", "-c", "echo before \\; env rm file"],
    ["printf", "%s", "bash -o pipefail -c 'rm file'"],
    ["echo", "sh -c 'env rm file'"],
    ["python3", "-c", "import os; os.remove('file')"],
    ["docker", "run", "fixture", "rm", "file"],
])
def test_benign_arguments_and_non_semantic_limits(argv):
    assert not declared_command_deletion(argv)


@pytest.mark.usefixtures("unjailed_gateway_command")
@pytest.mark.parametrize("read_only", [False, True])
@pytest.mark.parametrize("program", ["echo", "printf"])
@pytest.mark.parametrize("literal", [
    "rm file", "text; rm file", "text | rm file", "text\nrm file",
    "bash -o pipefail -c 'rm file'", "sh -c 'env rm file'",
    "env dash -c 'rm file'",
    "LABEL=fixture rm fixture", "if true; then env rm fixture; fi",
])
def test_literal_arguments_need_no_deletion_consent(development, monkeypatch, read_only, program, literal):
    root, store, worker, home = development
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, "request", lambda *a, **kw: pytest.fail("literal argument requested consent"))
    if read_only:
        monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    argv = [program, literal] if program == "echo" else [program, "%s", literal]
    receipt = gateway.dispatch(store, worker, "command_run", dict(argv=argv, request_key="literal"))
    assert receipt["exit_code"] == 0 and receipt["complete"], receipt
    assert receipt["output"] == literal + ("\n" if program == "echo" else "")
    assert receipt["profile"] == ("read_only" if read_only else "development")


def test_command_identity_does_not_collide():
    args = dict(argv=["python3", "-m", "pytest"], cwd="/tmp/one", profile="development")
    assert command_identity("command_run", args) != command_identity("command_run", dict(args, cwd="/tmp/two"))
    assert command_identity("command_run", args) != command_identity("command_run", dict(args, profile="restricted"))
    # The read-only profile and the Guarded writer profile are distinct runs.
    assert (command_identity("command_run", dict(args, profile="read_only"))
            != command_identity("command_run", dict(args, profile="restricted")))
    assert command_identity("command_run", args) != command_identity("command_run", dict(argv=args["argv"]))


@pytest.mark.usefixtures("unjailed_gateway_command")
def test_policy_switch_stops_owned_process_and_fences_retry(development):
    root, store, worker, home = development
    started = home / "started"
    args = dict(argv=[sys.executable, "-B", "-c", f"from pathlib import Path;import time;Path({str(started)!r}).write_text('yes');time.sleep(20)"],
                request_key="long")
    result = {}
    thread = threading.Thread(target=lambda: result.setdefault("receipt", gateway.dispatch(store, worker, "command_run", args)))
    thread.start()
    try:
        deadline = time.monotonic() + 5
        while not started.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        assert started.exists()
        access_mode.switch(store, worker.task_id, "guarded", origin="acp_set_mode")
    finally:
        thread.join(timeout=5)
    assert not thread.is_alive()
    assert result["receipt"]["failure"] == "execution_policy_changed"
    assert not result["receipt"]["complete"]
    # Return the retained incomplete outcome, never execute the command again.
    assert gateway.dispatch(store, worker, "command_run", args) == result["receipt"]


@pytest.mark.usefixtures("unjailed_gateway_command")
@pytest.mark.parametrize("target", ["guarded", "auto"])
def test_policy_switch_never_stops_a_read_only_roles_command(development, monkeypatch, target):
    """A read-only command's profile does not depend on the mode, so a mode
    switch while it runs (a reviewer's long test run) lets it finish."""
    root, store, worker, home = development
    monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    started, switched = home / "started", home / "switched"
    script = ("import pathlib, time\n"
              f"pathlib.Path({str(started)!r}).write_text('yes')\n"
              "deadline = time.monotonic() + 20\n"
              f"while not pathlib.Path({str(switched)!r}).exists() and time.monotonic() < deadline:\n"
              "    time.sleep(.02)\n"
              f"raise SystemExit(0 if pathlib.Path({str(switched)!r}).exists() else 9)\n")
    args = dict(argv=[sys.executable, "-B", "-c", script], request_key="reviewer-long")
    result = {}
    thread = threading.Thread(target=lambda: result.setdefault("receipt", gateway.dispatch(store, worker, "command_run", args)))
    thread.start()
    try:
        deadline = time.monotonic() + 5
        while not started.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        assert started.exists()
        access_mode.switch(store, worker.task_id, target, origin="acp_set_mode")
        time.sleep(.3)  # several of the runner's poll intervals
        switched.write_text("yes")
    finally:
        thread.join(timeout=25)
    assert not thread.is_alive()
    receipt = result["receipt"]
    assert receipt["failure"] is None and receipt["complete"] and receipt["exit_code"] == 0, receipt
    assert receipt["profile"] == "read_only"


@pytest.mark.requires_bwrap
def test_real_host_write_restricted_home_and_private_masks(development):
    root, store, worker, home = development
    outside = home / "sdk-state"
    private = root / ".cortex"
    script = f"from pathlib import Path;Path({str(outside)!r}).write_text('built');assert not Path({str(private / 'cortex.sqlite3')!r}).exists()"
    result = gateway.dispatch(store, worker, "command_run", dict(argv=[sys.executable, "-B", "-c", script], request_key="host"))
    assert result["exit_code"] == 0 and result["complete"], result
    assert outside.read_text() == "built"
    access_mode.switch(store, worker.task_id, "guarded", origin="acp_set_mode")
    script = f"from pathlib import Path\ntry: Path({str(outside)!r}).write_text('bad')\nexcept OSError: print('restricted')\nelse: raise AssertionError('host writable')"
    result = gateway.dispatch(store, worker, "command_run", dict(argv=[sys.executable, "-B", "-c", script], request_key="restricted"))
    assert result["exit_code"] == 0 and result["complete"], result
    assert outside.read_text() == "built"


@pytest.mark.parametrize("mode", ["development", "auto"])
def test_private_cwd_alias_and_read_only_deletion_are_refused(development, monkeypatch, mode):
    root, store, worker, home = development
    access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
    alias = home / "alias"
    alias.symlink_to(root / ".cortex", target_is_directory=True)
    with pytest.raises((StoreError, OSError)):
        gateway.dispatch(store, worker, "command_run", dict(argv=["true"], cwd=str(alias), request_key="alias"))
    monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    with pytest.raises(StoreError, match="gateway_scope_denied"):
        gateway.dispatch(store, worker, "command_run", dict(argv=["rm", "fixture"], request_key="readonly-delete"))


@pytest.mark.usefixtures("unjailed_gateway_command")
@pytest.mark.parametrize("mode", ["development", "auto", "guarded"])
def test_read_only_role_command_is_restricted_in_every_mode(development, monkeypatch, mode):
    root, store, worker, home = development
    access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
    monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    used = []
    real = sandbox.readonly_command
    monkeypatch.setattr(sandbox, "readonly_command", lambda *a, **kw: used.append(mode) or real(*a, **kw))
    monkeypatch.setattr(sandbox, "development_command", lambda *a, **kw: pytest.fail("host-writable profile"))
    # The mode still decides whether a cwd outside the task tree is allowed;
    # the host stays read-only either way.
    if mode == "guarded":
        with pytest.raises(StoreError, match="gateway_command_cwd_denied"):
            gateway.dispatch(store, worker, "command_run", dict(argv=["true"], cwd=str(home), request_key="outside"))
    else:
        receipt = gateway.dispatch(store, worker, "command_run", dict(argv=["true"], cwd=str(home), request_key="outside"))
        assert receipt["complete"] and receipt["cwd"] == str(home) and receipt["profile"] == "read_only", receipt
    receipt = gateway.dispatch(store, worker, "command_run", dict(argv=["true"], request_key="inside"))
    assert receipt["profile"] == "read_only" and receipt["complete"], receipt
    assert used == [mode] * (1 if mode == "guarded" else 2)


@pytest.mark.requires_bwrap
@pytest.mark.parametrize("mode", ["development", "auto", "guarded"])
def test_real_read_only_role_writes_caches_not_host_or_project(development, monkeypatch, tmp_path, mode):
    root, store, worker, home = development
    access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
    monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    (home / ".bashrc").write_text("# owner\n")
    (home / ".nvm").mkdir()
    sibling = tmp_path / "sibling-project"
    sibling.mkdir()
    activity = home / ".docker/buildx/activity"
    activity.mkdir(parents=True)
    refused = [home / ".bashrc", home / ".profile", home / ".nvm/node", home / "sdk-state"]
    # A project write lands in the discarded overlay (refused where the host
    # cannot overlay); either way the project keeps nothing.
    discarded = root / "reviewer-wrote"
    # The host /tmp is deliberately writable, including this sibling fixture.
    allowed = [home / ".cache/tool/entry", home / ".npm/_cacache/entry", activity / "default",
               sibling / "planted"]
    script = (
        "import os, tempfile\nfrom pathlib import Path\n"
        f"for name in {[str(p) for p in refused]!r}:\n"
        "    try: Path(name).write_text('bad')\n"
        "    except OSError: pass\n"
        "    else: raise AssertionError('writable: ' + name)\n"
        f"for name in {[str(p) for p in allowed]!r}:\n"
        "    Path(name).parent.mkdir(parents=True, exist_ok=True); Path(name).write_text('ok')\n"
        f"try: Path({str(discarded)!r}).write_text('discarded')\n"
        "except OSError: pass\n"
        "with tempfile.NamedTemporaryFile() as handle: handle.write(b'ok')\n"
        # Namespace identity avoids a host PID colliding with an unrelated local PID.
        f"assert os.readlink('/proc/self/ns/pid') != {os.readlink('/proc/self/ns/pid')!r}, 'host PID namespace'\n"
        "assert os.readlink('/proc/1/ns/pid') == os.readlink('/proc/self/ns/pid'), 'host proc mount'\n"
        "print('read-only host')\n")
    result = gateway.dispatch(store, worker, "command_run", dict(argv=[sys.executable, "-B", "-c", script], request_key="ro"))
    assert result["exit_code"] == 0 and result["complete"] and result["profile"] == "read_only", result
    assert result["output"].strip() == "read-only host"
    assert (home / ".bashrc").read_text() == "# owner\n"
    assert not any(path.exists() for path in refused[1:])
    assert not discarded.exists()
    assert all(path.read_text() == "ok" for path in allowed)


@pytest.mark.parametrize("path", [None, "", "/fixture/bin"])
def test_owner_path_defaults_only_when_unset(tmp_path, path):
    root = tmp_path / "project"
    root.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    home = tmp_path / "home"
    home.mkdir()
    (root / ".venv/bin").mkdir(parents=True)
    environ = {"HOME": str(home)}
    if path is not None:
        environ["PATH"] = path
    with sandbox.development_command(root, scratch, ["/bin/sh"], ["."],
                                     environ=environ) as (command, _):
        paths = [command[i + 2] for i in range(len(command) - 2)
                 if command[i:i + 2] == ["--setenv", "PATH"]]
        entries = paths[-1].split(":")
        assert entries[0] == str(root / ".venv/bin")
        expected = "/usr/local/bin:/usr/bin:/bin" if path is None else path
        if expected:
            assert entries[1:1 + len(expected.split(":"))] == expected.split(":")
        else:
            assert len(entries) == 1


@pytest.mark.requires_bwrap
def test_real_read_only_role_runs_tests_and_git_reads(development, monkeypatch):
    root, store, worker, home = development
    monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    monkeypatch.setenv("PYTHONPATH", str(Path(pytest.__file__).parent.parent))
    (root / "test_fixture_suite.py").write_text(
        "def test_cache(pytestconfig, tmp_path):\n"
        "    pytestconfig.cache.set('example/value', 1)\n"
        "    (tmp_path / 'scratch').write_text('ok')\n")
    result = gateway.dispatch(store, worker, "command_run", dict(
        argv=[sys.executable, "-B", "-m", "pytest", "-q", "test_fixture_suite.py"], request_key="pytest"))
    assert result["exit_code"] == 0 and result["complete"], result
    assert not (root / ".pytest_cache").exists()
    import subprocess
    identity = dict(os.environ, GIT_AUTHOR_NAME="fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                    GIT_COMMITTER_NAME="fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
    if not (root / ".git").exists():
        subprocess.run(["git", "init", "-q"], cwd=root, check=True, env=identity)
    subprocess.run(["git", "add", "test_fixture_suite.py"], cwd=root, check=True, env=identity)
    subprocess.run(["git", "commit", "-q", "-m", "fixture"], cwd=root, check=True, env=identity)
    result = gateway.dispatch(store, worker, "command_run", dict(
        argv=["sh", "-c", "git status --short >/dev/null && git log -1 --format=%s && git diff --check"],
        request_key="git"))
    assert result["exit_code"] == 0 and result["complete"], result
    assert result["output"].strip() == "fixture"


@pytest.mark.parametrize("mode,origin", [("auto", "model"), ("unknown", "acp_set_mode"), ("", "acp_set_mode")])
def test_corrupt_session_selection_fails_closed(tmp_path, mode, origin):
    store, task_id = _store_and_task(tmp_path)
    sessions.create_session(store, "fresh", store.project_root, {}, {}, [])
    with store.connection() as db:
        db.execute("PRAGMA ignore_check_constraints=ON")
        db.execute("UPDATE acp_session_access_defaults SET selected_mode=?,selection_origin=?", (mode, origin))
        db.commit()
    assert sessions.session_access_mode(store, "fresh") == "guarded"
    sessions.bind_task(store, "fresh", task_id)
    assert access_mode.current(store, task_id) == "guarded"


@pytest.mark.usefixtures("unjailed_gateway_command")
def test_confirmed_outside_command_filters_runtime_credentials(development, monkeypatch):
    root, store, worker, home = development
    from cortex_runtime.orchestration import consent
    monkeypatch.setenv("CORTEX_CHANNEL_SECRET", "fixture-secret")
    monkeypatch.setenv("PRIVATE_TOKEN", "fixture-token")
    monkeypatch.setattr(consent, "request", lambda *a, **kw: True)
    receipt = gateway.dispatch(store, worker, "outside_command", dict(
        argv=[sys.executable, "-B", "-c", "import os;assert 'CORTEX_CHANNEL_SECRET' not in os.environ;assert 'PRIVATE_TOKEN' not in os.environ;print('filtered')"],
        cwd=str(home), request_key="confirmed"))
    assert receipt["exit_code"] == 0 and receipt["complete"], receipt
    assert receipt["output"] == "filtered\n"


def test_revoked_development_channel_never_launches(development, monkeypatch):
    root, store, worker, home = development
    with store.connection() as db:
        db.execute("UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?", (store._now(), worker.channel_id))
        db.commit()
    monkeypatch.setattr(gateway.subprocess, "Popen", lambda *a, **kw: pytest.fail("revoked launch"))
    with pytest.raises(StoreError, match="gateway_channel_inactive"):
        gateway.dispatch(store, worker, "command_run", dict(argv=["true"], request_key="revoked"))


def test_worktree_under_protected_parent_keeps_ordered_task_grant(tmp_path):
    home = tmp_path / "home"
    root = home / ".paseo/worktrees/project"
    root.mkdir(parents=True)
    (root / ".cortex").mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    with sandbox.development_command(root, scratch, ["true"], ["."], environ={"HOME": str(home)}) as (command, fds):
        parent = command.index(str(home / ".paseo"))
        grant = next(i for i in range(len(command)-2) if command[i] == "--bind" and command[i+2] == str(root))
        assert parent < grant
        assert any(command[i:i+2] == ["--tmpfs", str(root / ".cortex")] for i in range(grant, len(command)-1))



@pytest.mark.parametrize("mode", ["development", "auto"])
def test_auto_and_development_share_the_host_command_profile(development, monkeypatch, mode):
    root, store, worker, home = development
    access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
    used = []
    real = sandbox.development_command
    monkeypatch.setattr(sandbox, "development_command", lambda *a, **kw: used.append(mode) or real(*a, **kw))
    monkeypatch.setattr(gateway, "_run_command_process", lambda *a, **kw: (0, None))
    receipt = gateway.dispatch(store, worker, "command_run", dict(argv=["true"], request_key=mode))
    assert used == [mode] and receipt["profile"] == "development"
    assert receipt["access_policy"]["mode"] == mode


@pytest.mark.parametrize("mode", ["development", "auto"])
@pytest.mark.parametrize("relative", [".claude/settings.json", ".codex/config.toml", ".gitconfig",
                                      ".config/cortex/defaults.json", ".paseo/plugins.json", "link/settings.json"])
def test_outside_write_to_control_plane_still_asks(development, monkeypatch, mode, relative):
    root, store, worker, home = development
    access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
    (home / ".claude").mkdir()
    (home / "link").symlink_to(home / ".claude", target_is_directory=True)
    target = home / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    from cortex_runtime.orchestration import consent
    asked = []
    monkeypatch.setattr(consent, "request", lambda *a, **kw: asked.append(a[2]) or False)
    with pytest.raises(StoreError, match="outside_action_not_confirmed"):
        gateway.dispatch(store, worker, "outside_write", dict(path=str(target), content="{}", request_key="cp"))
    assert asked == ["outside_write"]
    assert not target.exists() and not (home / ".claude/settings.json").exists()


@pytest.mark.parametrize("mode", ["development", "auto"])
def test_outside_command_keeps_its_card_in_host_wide_modes(development, monkeypatch, mode):
    root, store, worker, home = development
    access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
    from cortex_runtime.orchestration import consent
    asked = []
    monkeypatch.setattr(consent, "request", lambda *a, **kw: asked.append(a[2]) or False)
    monkeypatch.setattr(gateway.subprocess, "Popen", lambda *a, **kw: pytest.fail("ran without a card"))
    with pytest.raises(StoreError, match="outside_action_not_confirmed"):
        gateway.dispatch(store, worker, "outside_command", dict(argv=["true"], cwd=str(home), request_key="oc"))
    assert asked == ["outside_command"]


def test_real_read_only_role_runs_a_suite_that_writes_inside_the_project(development, monkeypatch):
    """vitest writes node_modules/.vite-temp and Playwright its results; a
    reviewer's suite failed with EROFS on them (2026-10-21)."""
    from cortex_runtime.orchestration import sandbox
    if not sandbox.tmp_overlay_supported():
        pytest.skip("bubblewrap cannot create a tmp overlay on this host")
    root, store, worker, _ = development
    monkeypatch.setattr(gateway, "_authority", lambda db, ch: ([], "read-only"))
    (root / "node_modules").mkdir()
    script = ("from pathlib import Path\n"
              "Path('node_modules/.vite-temp').mkdir()\n"
              "Path('node_modules/.vite-temp/config.mjs').write_text('x')\n"
              "Path('test-results').mkdir(); Path('test-results/out.json').write_text('{}')\n"
              "print('suite ran')\n")
    result = gateway.dispatch(store, worker, "command_run",
                              dict(argv=[sys.executable, "-B", "-c", script], request_key="suite"))
    assert result["exit_code"] == 0 and result["output"].strip() == "suite ran", result
    assert result["profile"] == "read_only"
    assert not (root / "node_modules/.vite-temp").exists() and not (root / "test-results").exists()
