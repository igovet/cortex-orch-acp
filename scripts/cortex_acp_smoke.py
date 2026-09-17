#!/usr/bin/env python3
"""No-paid startup smoke harness for the production incoming ACP agent.

Spawns the real ``plugins/cortex/scripts/cortex_acp.py`` entry point (never
a test-only fake runtime -- see ``tests/acp_fakes.py`` for the fake used by
pytest) against a fresh temporary project directory and a fresh temporary
``HOME``/``XDG_CONFIG_HOME``/``XDG_STATE_HOME``, using a minimal, explicitly
built process environment (never a copy of this process's full environment)
so no host credential or unrelated variable is ever inherited by the agent
subprocess.

By default no connection exists, so the agent must advertise no ``model``
config option and no real orchestration is reachable at all (the production
entry point always runs with ``runtime.UnavailableRuntime``, which raises
``OrchestrationUnavailable`` for any real prompt -- see
``cortex_runtime.acp.runtime`` and ``cortex_runtime.acp.agent.main``). This
harness never sends a prompt that could reach a paid model and never
constructs or reads any real user connection/catalog state
(``~/.config/cortex``, ``~/.local/state/cortex`` are never touched -- only
the fresh temporary directories created here).

``--fixture-connection`` additionally creates one fake, clearly-fake
connection (fake API key text, placeholder model ids) and a placeholder
catalog snapshot -- entirely inside the temporary ``XDG_*`` directories
created for this run -- so the ``model``/``thought_level`` config-option
rendering path can be exercised too. This never contacts any real engine or
provider.

This script never uses this repository's own checkout as an ACP project
root (see ``tests/conftest.py``'s T009-incident guard for the same rule
applied to the pytest suite): the ACP ``session/new`` ``cwd`` is always a
freshly created temporary directory outside this checkout.

Exit code: ``0`` iff every check below passed. A JSON summary report
(python/plugin/protocol versions, and one entry per check) is written to
``--report PATH`` if given, else printed to stderr.

Known limitation (F5, T011 review): this harness only observes the ACP
protocol stream on stdout and the agent's exit code; it captures the
subprocess's stderr for failure diagnostics but does not itself assert
anything about pre-``sys.stdout = sys.stderr`` redirect output (see
``cortex_runtime.acp.agent.main``, which repoints ``sys.stdout`` to
``sys.stderr`` *before* constructing the ``Agent``/transport). Any stray
``print()`` a future change accidentally leaves *before* that redirect
line -- which would land on the real stdout, ahead of the first protocol
frame -- is not something this harness can distinguish from a client
itself misbehaving, because by the time this harness's own reader thread
starts consuming stdout, the write already happened. A real client's own
stdout parsing during T035 GUI qualification, or a manual
``python3 -B plugins/cortex/scripts/cortex_acp.py < /dev/null | head -c 200
| xxd`` byte-level check, remains the authoritative check for this
specific failure mode; see ``docs/acp/real-client-runbook.md``'s
stdout-clean-verification row.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import queue
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS_DIR = REPO_ROOT / "plugins" / "cortex" / "scripts"
AGENT_ENTRY = SCRIPTS_DIR / "cortex_acp.py"
SCHEMA_PATH = REPO_ROOT / "tests" / "fixtures" / "acp" / "schema-1.4.0.json"
PLUGIN_JSON = REPO_ROOT / "plugins" / "cortex" / ".codex-plugin" / "plugin.json"

READ_TIMEOUT_SECONDS = 10.0
PROC_EXIT_TIMEOUT_SECONDS = 15.0


class SmokeFailure(RuntimeError):
    """Raised for a hard-stop failure inside a single check."""


class Report:
    def __init__(self):
        self.checks: list[dict] = []

    def record(self, name, ok, detail=None):
        entry = dict(name=name, status="pass" if ok else "fail")
        if detail is not None:
            entry["detail"] = detail
        self.checks.append(entry)
        return ok

    def skip(self, name, reason):
        self.checks.append(dict(name=name, status="skip", detail=reason))

    @property
    def all_passed(self):
        return all(check["status"] in ("pass", "skip") for check in self.checks)


def _plugin_version():
    try:
        return json.loads(PLUGIN_JSON.read_text())["version"]
    except (OSError, ValueError, KeyError):
        return "unknown"


def _minimal_env(home, config_home, state_home):
    """An explicitly built, minimal process environment -- never a copy of
    ``os.environ`` (unlike ``tests/acp_fakes.py``, which does copy the full
    real environment for pytest's own convenience): this harness exists in
    part to demonstrate the agent needs nothing beyond these few variables
    and inherits no host credential."""
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(config_home),
        "XDG_STATE_HOME": str(state_home),
        "LANG": os.environ.get("LANG") or "C.UTF-8",
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    return env


def _build_fixture_connection(environ):
    """Create one fake connection + placeholder catalog snapshot entirely
    inside ``environ``'s ``XDG_CONFIG_HOME``/``XDG_STATE_HOME`` (never the
    real user directories). Fake secret text only, never a live probe.

    ``environ`` is passed explicitly all the way down to every filesystem
    path this fixture touches (``cortex_runtime.connections.registry``/
    ``onboarding``); it is never inferred from this harness process's own
    ``os.environ``, so this works correctly regardless of the real
    process's ``$HOME``/``$XDG_*`` -- including a real ``$HOME`` that is
    itself a Git repository (T011 follow-up: previously
    ``onboarding._derive_credential_ref`` silently dropped ``environ`` on
    three internal ``registry.canonical_credential_ref`` calls and fell
    back to the real process environment; fixed in
    ``cortex_runtime.connections.onboarding``/``registry``, see
    ``tests/test_connections_registry.py``).
    """
    sys.path.insert(0, str(SCRIPTS_DIR))
    from cortex_runtime.catalog import discovery, store as catalog_store
    from cortex_runtime.connections import onboarding
    from cortex_runtime.connections.model import AuthMode

    record = onboarding.create_connection(
        name="Smoke Fixture", engine="codex-acp", provider="openai",
        auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
        endpoint="cortex-smoke:fixture", environ=environ)
    record = onboarding.set_api_key(record.id, reader=lambda: b"fake-smoke-secret-key", environ=environ)
    options = [
        dict(id="model", name="Model", type="select", category="model",
             currentValue="smoke-placeholder-model",
             options=[dict(value="smoke-placeholder-model", name="Smoke Placeholder Model")]),
        dict(id="thought_level", name="Thought level", type="select", category="thought_level",
             currentValue="high", options=[dict(value="high", name="high"), dict(value="low", name="low")]),
    ]
    observation = discovery.parse_observation(options)
    snapshot = discovery.build_catalog({record.id: record}, {record.id: observation})
    catalog_store.save_snapshot(snapshot, environ)
    return record


class AgentSession:
    """Owns the spawned subprocess and framed JSON-RPC I/O over stdio.

    stdout is drained continuously by a background thread into a queue: a
    bare ``select()`` on the pipe's file descriptor is not safe to mix with
    Python's buffered ``TextIOWrapper.readline()`` (a single ``readline()``
    call can pull more than one already-available line into the userspace
    buffer, after which the fd itself reports no further pending bytes even
    though a full second line is already sitting in the buffer -- the
    reader thread avoids that entirely by always calling ``readline()``
    itself, never ``select``, and handing complete lines across the queue).
    """

    def __init__(self, env, cwd):
        self.proc = subprocess.Popen(
            [sys.executable, "-B", str(AGENT_ENTRY)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, env=env, cwd=str(cwd))
        self._lines: "queue.Queue[str | None]" = queue.Queue()
        self._stderr_lines: list[str] = []
        self._reader = threading.Thread(target=self._read_stdout, daemon=True)
        self._reader.start()
        self._stderr_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self._stderr_reader.start()

    def _read_stdout(self):
        try:
            for line in self.proc.stdout:
                self._lines.put(line.rstrip("\n"))
        finally:
            self._lines.put(None)

    def _read_stderr(self):
        try:
            for line in self.proc.stderr:
                self._stderr_lines.append(line.rstrip("\n"))
        except Exception:
            pass

    def send(self, message):
        line = json.dumps(message)
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()

    def recv(self, timeout=READ_TIMEOUT_SECONDS):
        try:
            line = self._lines.get(timeout=timeout)
        except queue.Empty:
            raise SmokeFailure(
                f"timed out after {timeout}s waiting for a stdout line "
                f"(stderr so far: {self._drain_stderr()!r})") from None
        if line is None:
            raise SmokeFailure(f"agent closed stdout unexpectedly (stderr: {self._drain_stderr()!r})")
        try:
            return line, json.loads(line)
        except json.JSONDecodeError as exc:
            raise SmokeFailure(f"non-JSON stdout line {line!r}: {exc}") from exc

    def _drain_stderr(self):
        return "\n".join(self._stderr_lines)

    def close(self, timeout=PROC_EXIT_TIMEOUT_SECONDS):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=timeout)
        return self.proc.returncode


def _load_validators():
    try:
        import jsonschema
    except ImportError:
        return None
    document = json.loads(SCHEMA_PATH.read_text())
    resolver = jsonschema.RefResolver.from_schema(document)

    def _validator(def_name):
        return jsonschema.Draft202012Validator({"$ref": f"#/$defs/{def_name}"}, resolver=resolver)

    return dict(response=_validator("AgentResponse"), notification=_validator("AgentNotification"))


def _validate(report, validators, kind, message, label):
    if validators is None:
        report.skip(f"schema:{label}", "jsonschema not importable; schema validation skipped")
        return
    try:
        validators[kind].validate(message)
        report.record(f"schema:{label}", True)
    except Exception as exc:  # jsonschema.ValidationError and friends
        report.record(f"schema:{label}", False, detail=str(exc))


def _count_tasks(project_dir):
    db_path = project_dir / ".cortex" / "cortex.sqlite3"
    if not db_path.exists():
        return 0
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True, timeout=5)
    try:
        return conn.execute("SELECT count(*) FROM tasks").fetchone()[0]
    finally:
        conn.close()


def run_smoke(*, fixture_connection, report):
    project_dir = Path(tempfile.mkdtemp(prefix="cortex-acp-smoke-project-")).resolve()
    home_dir = Path(tempfile.mkdtemp(prefix="cortex-acp-smoke-home-")).resolve()
    assert REPO_ROOT not in project_dir.parents and project_dir != REPO_ROOT, (
        "refusing to use the repository checkout as an ACP project root")
    config_home = home_dir / "config"
    state_home = home_dir / "state"
    config_home.mkdir(parents=True, exist_ok=True)
    state_home.mkdir(parents=True, exist_ok=True)
    env = _minimal_env(home_dir, config_home, state_home)

    if fixture_connection:
        _build_fixture_connection(dict(XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home),
                                        HOME=str(home_dir)))

    validators = _load_validators()
    if validators is None:
        sys.stderr.write("warning: jsonschema not importable; schema-validation checks will be skipped\n")

    session = AgentSession(env, project_dir)
    protocol_version_seen = None
    try:
        # -- initialize -------------------------------------------------
        session.send(dict(jsonrpc="2.0", id=1, method="initialize",
                           params=dict(protocolVersion=1, clientCapabilities={},
                                       clientInfo=dict(name="cortex-acp-smoke", version="1"))))
        _line, init_reply = session.recv()
        _validate(report, validators, "response", init_reply, "initialize")
        result = init_reply.get("result", {})
        protocol_version_seen = result.get("protocolVersion")
        report.record("initialize:protocol_version_1", protocol_version_seen == 1,
                       detail=protocol_version_seen)
        caps = result.get("agentCapabilities", {})
        report.record("initialize:loadSession_false", caps.get("loadSession") is False, detail=caps)
        prompt_caps = caps.get("promptCapabilities", {})
        report.record(
            "initialize:text_only_prompt_capabilities",
            prompt_caps == dict(image=False, audio=False, embeddedContext=False),
            detail=prompt_caps)

        # -- session/new --------------------------------------------------
        session.send(dict(jsonrpc="2.0", id=2, method="session/new",
                           params=dict(cwd=str(project_dir), mcpServers=[])))
        _line, new_reply = session.recv()
        _validate(report, validators, "response", new_reply, "session/new")
        new_result = new_reply.get("result", {})
        session_id = new_result.get("sessionId")
        report.record("session/new:has_session_id", isinstance(session_id, str) and bool(session_id))

        config_options = new_result.get("configOptions")
        if fixture_connection:
            ids = {opt.get("id") for opt in (config_options or [])}
            report.record("session/new:configOptions_with_fixture", {"model", "thought_level"} <= ids,
                           detail=config_options)
        else:
            report.record("session/new:no_model_option_without_connections", config_options is None,
                           detail=config_options)

        _line, commands_notification = session.recv()
        _validate(report, validators, "notification", commands_notification, "available_commands_update")
        update = commands_notification.get("params", {}).get("update", {})
        names = {entry.get("name") for entry in update.get("availableCommands", [])}
        report.record("available_commands:cortex_status_and_setup_present",
                       {"cortex-status", "cortex-setup"} <= names, detail=sorted(n for n in names if n))

        # -- /cortex-status: no task created, no orchestration call -------
        request_id_status = 3
        session.send(dict(jsonrpc="2.0", id=request_id_status, method="session/prompt",
                           params=dict(sessionId=session_id,
                                       prompt=[dict(type="text", text="/cortex-status")])))
        _line, status_chunk = session.recv()
        _validate(report, validators, "notification", status_chunk, "cortex_status_chunk")
        report.record("cortex_status:agent_message_chunk",
                       status_chunk.get("params", {}).get("update", {}).get("sessionUpdate") == "agent_message_chunk")
        _line, status_reply = session.recv()
        _validate(report, validators, "response", status_reply, "cortex_status_result")
        report.record("cortex_status:end_turn",
                       status_reply.get("result", {}).get("stopReason") == "end_turn", detail=status_reply)
        task_count_after_status = _count_tasks(project_dir)
        report.record("cortex_status:no_task_created", task_count_after_status == 0,
                       detail=task_count_after_status)

        # -- a normal prompt must fail closed with no hidden provider call
        request_id_prompt = 4
        session.send(dict(jsonrpc="2.0", id=request_id_prompt, method="session/prompt",
                           params=dict(sessionId=session_id,
                                       prompt=[dict(type="text", text="Please write a haiku.")])))
        _line, prompt_reply = session.recv()
        _validate(report, validators, "response", prompt_reply, "normal_prompt_result")
        error = prompt_reply.get("error", {})
        report.record("normal_prompt:orchestration_unavailable",
                       error.get("code") == -32001 and error.get("message") == "orchestration_unavailable",
                       detail=error)
    finally:
        returncode = session.close()
        report.record("process:clean_exit_0", returncode == 0, detail=returncode)

    return dict(python_version=platform.python_version(), cortex_plugin_version=_plugin_version(),
                protocol_version=protocol_version_seen)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture-connection", action="store_true",
                         help="Create one fake connection + placeholder catalog snapshot in the "
                              "temporary XDG directories (never a real engine/provider).")
    parser.add_argument("--report", type=Path, default=None,
                         help="Write the JSON summary report here instead of stderr.")
    args = parser.parse_args(argv)

    report = Report()
    versions = dict(python_version=platform.python_version(), cortex_plugin_version=_plugin_version(),
                     protocol_version=None)
    try:
        versions = run_smoke(fixture_connection=args.fixture_connection, report=report)
    except SmokeFailure as exc:
        report.record("harness:fatal", False, detail=str(exc))
    except Exception as exc:  # unexpected: still emit a report before exiting non-zero
        report.record("harness:unexpected_exception", False, detail=f"{type(exc).__name__}: {exc}")

    summary = dict(versions=versions, checks=report.checks, all_passed=report.all_passed,
                    run_id=str(uuid.uuid4()), timestamp=time.time())
    text = json.dumps(summary, indent=2)
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(text + "\n")
    else:
        sys.stderr.write(text + "\n")
    return 0 if report.all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
