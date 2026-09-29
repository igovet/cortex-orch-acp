"""OpenCode v2 as an optional, fail-closed engine: unit checks that need no OpenCode binary.

The installed-binary checks live in ``test_engine_opencode_qualification.py``.
"""
from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import time
from pathlib import Path

import pytest

from cortex_runtime.configuration.policy import COORDINATOR_MODE_FLOOR, WORKER_MODE_FLOOR
from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.connections import discovery, environment, model
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.delegation import generations
from cortex_runtime.delegation.live_adapters import (
    _option_mapping_for, _pinned_version_for, _session_new_meta_for, base_prompt_mode,
)
from cortex_runtime.engine import opencode_adapter_acp, opencode_install
from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.engine.mcp_bridge import host_mcp_servers
from cortex_runtime.engine.process import EngineProcess
from cortex_runtime.orchestration.sandbox import engine_jail
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute
from cortex_runtime.portable.instruction_delivery import DeliveryQualification
from cortex_runtime.portable.roles import RoleInstructionSet  # noqa: F401 - import proves the portable seam loads
from engine_fakes import isolated_environ, make_connection_record, new_connection_id

needs_bwrap = pytest.mark.skipif(shutil.which("bwrap") is None, reason="bubblewrap is required")


def _fake_binary(tmp_path, version="2.0.19", mode=0o755):
    """A stand-in `opencode` that only answers `--version`."""
    path = tmp_path / "bin" / "opencode"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\necho 'opencode v{version}'\n")
    path.chmod(mode)
    return path


def _revision(connection_id, model_id="prov/m1", parameters=()):
    route = ExecutionRoute(engine="opencode", connection_id=connection_id, model=model_id,
                           supported_parameters=parameters)
    return ConfigurationRevision(id="cr_" + "a" * 12, task_id="t_" + "b" * 12, revision=1, route=route)


def _options(current_model="prov/m1", efforts=("low", "high", "default")):
    options = [
        dict(id="model", category="model", type="select", currentValue=current_model,
             options=[dict(value="prov/m1", name="M1"), dict(value="prov/m2", name="M2")]),
        dict(id="mode", category="mode", type="select", currentValue="cortex", options=[dict(value="cortex", name="cortex")]),
    ]
    if efforts:
        options.append(dict(id="effort", category="thought_level", type="select", currentValue="default",
                            options=[dict(value=e, name=e) for e in efforts]))
    return options


# --- registration -------------------------------------------------------------

def test_opencode_is_registered_for_native_login_only():
    assert "opencode" in model.SUPPORTED_ENGINES
    assert model.validate_auth_selection("opencode", AuthMode.NATIVE_ACCOUNT, None).qualified
    for mode in (AuthMode.API_KEY, AuthMode.GATEWAY):
        with pytest.raises(PortableError):
            model.validate_auth_selection("opencode", mode, "openai_api_key" if mode == AuthMode.API_KEY else None)


def test_only_the_observed_release_is_qualified_and_startup_never_discovers_it(tmp_path):
    assert discovery.QUALIFIED_ADAPTER_VERSIONS["opencode"] == ("2.0.19",)
    assert discovery.is_qualified("opencode", "2.0.19")
    assert not discovery.is_qualified("opencode", "2.0.20")
    assert "opencode" not in discovery.SUPPORTED_ENGINES
    environ = {"HOME": str(tmp_path), "PATH": "/nonexistent"}
    assert "opencode" not in discovery.discover_all(environ=environ, node_binary="/nonexistent/node")
    assert discovery.discover_opencode(environ=environ) is None


def test_modes_tiers_and_delivery_are_registered_for_exactly_2_0_19():
    assert WORKER_MODE_FLOOR["opencode"] == COORDINATOR_MODE_FLOOR["opencode"] == opencode_adapter_acp.AGENT
    assert generations.tier_for("opencode", "2.0.19").tier == generations.CONTROLLED_BOUNDARY
    plan = delivery_for("opencode", "2.0.19").plan(type("Role", (), {"body_portable": "role"})(), [])
    assert plan.qualification == DeliveryQualification.QUALIFIED
    assert plan.blocks == () and plan.session_new_meta is None
    other = delivery_for("opencode", "2.0.20").plan(type("Role", (), {"body_portable": "role"})(), [])
    assert other.qualification == DeliveryQualification.UNSUPPORTED
    assert _pinned_version_for("opencode") == "2.0.19"
    # Nothing is sent as session/new _meta: the role goes in at launch.
    assert _session_new_meta_for("opencode", "2.0.19", "role text") is None


def test_host_mcp_entries_may_name_opencode():
    entries = [dict(name="index", command="/bin/true", engines=["opencode"]),
               dict(name="other", command="/bin/true", engines=["codex-acp"])]
    assert [e["name"] for e in host_mcp_servers(entries, engine="opencode")] == ["index"]


# --- process configuration and prompt ----------------------------------------

def test_process_configuration_is_cortex_owned_and_names_no_provider():
    config = opencode_adapter_acp.build_opencode_config("/tmp/x/cortex-opencode-agent.md")
    assert config["default_agent"] == "cortex"
    assert config["agent"]["cortex"] == {"mode": "primary", "prompt": "{file:/tmp/x/cortex-opencode-agent.md}"}
    assert all(config["agent"][name] == {"disable": True} for name in ("build", "plan", "title"))
    assert config["permission"] == {"*": "ask", "task": "deny", "question": "deny", "skill": "deny"}
    assert config["autoupdate"] is False and config["share"] == "disabled"
    assert not {"provider", "model", "small_model", "mcp", "plugin", "instructions"} & set(config)
    for bad in ("relative.md", "/tmp/x}/a.md"):
        with pytest.raises(ValueError):
            opencode_adapter_acp.build_opencode_config(bad)
    env = opencode_adapter_acp.process_environment("/tmp/x/p.md")
    assert set(env) == {"OPENCODE_CONFIG_CONTENT", "OPENCODE_DISABLE_PROJECT_CONFIG", "OPENCODE_DISABLE_AUTOUPDATE"}
    assert json.loads(env["OPENCODE_CONFIG_CONTENT"])["agent"]["cortex"]["prompt"] == "{file:/tmp/x/p.md}"


def test_agent_prompt_carries_base_prompt_code_mode_note_role_and_project_rules(tmp_path):
    (tmp_path / "AGENTS.md").write_text("RULES-MARKER\n")
    prompt = opencode_adapter_acp.build_agent_prompt("ROLE-MARKER", role="worker", project_root=str(tmp_path),
                                                     environ={})
    assert prompt.startswith("# Cortex session\n\n")
    assert prompt.index(opencode_adapter_acp.CODE_MODE_NOTE) < prompt.index("ROLE-MARKER") < prompt.index("RULES-MARKER")
    native = opencode_adapter_acp.build_agent_prompt("ROLE", role="coordinator", project_root=None,
                                                     environ={"CORTEX_UNIFIED_BASE_PROMPT": "0"})
    assert not native.startswith("# Cortex session") and "ROLE" in native
    assert base_prompt_mode({"opencodeAgentPrompt": prompt}) == "unified"
    assert base_prompt_mode({"opencodeAgentPrompt": native}) == "native"
    with pytest.raises(ValueError):
        opencode_adapter_acp.build_agent_prompt(" ", role="worker", project_root=None)


# --- option mapping -----------------------------------------------------------

def test_option_mapping_sets_model_then_effort_and_only_the_cortex_mode():
    mapping = _option_mapping_for("opencode")
    assert mapping is opencode_adapter_acp.opencode_option_mapping
    cid = new_connection_id()
    assert mapping(_revision(cid), _options()) == [("model", "prov/m1")]
    params = (("thought_level", "high"), ("mode", "cortex"))
    assert mapping(_revision(cid, parameters=params), _options()) == [
        ("model", "prov/m1"), ("effort", "high"), ("mode", "cortex")]
    # The current model offers no such effort: refused before any wire call.
    with pytest.raises(ConfigurationRejected):
        mapping(_revision(cid, parameters=(("thought_level", "xhigh"),)), _options())
    # A different current model (whose efforts differ): the effort is left to
    # configure()'s re-check after the model switch.
    assert mapping(_revision(cid, "prov/m2", (("thought_level", "high"),)), _options(efforts=())) == [
        ("model", "prov/m2"), ("effort", "high")]
    with pytest.raises(ConfigurationRejected):
        mapping(_revision(cid, "prov/unknown"), _options())
    with pytest.raises(ConfigurationRejected):
        mapping(_revision(cid, parameters=(("mode", "build"),)), _options())


# --- environment --------------------------------------------------------------

def test_process_environment_isolates_data_dir_and_accepts_only_opencode_pins(tmp_path):
    environ = isolated_environ(tmp_path)
    record = make_connection_record(new_connection_id(), ("/usr/bin/true", "acp"), engine="opencode", environ=environ)
    pins = opencode_adapter_acp.process_environment("/tmp/p.md")
    env = environment.build_process_env(record, {"PATH": "/usr/bin", "HOME": environ["HOME"]}, environ=environ,
                                        pinned_engine_vars=pins)
    data_dir = Path(environment.credential_directory(record, environ))
    assert data_dir.name == "opencode"
    assert env["XDG_DATA_HOME"] == str(data_dir.parent)
    assert environment.engine_state_dir("opencode", env) == str(data_dir)
    assert env["OPENCODE_DISABLE_PROJECT_CONFIG"] == env["OPENCODE_DISABLE_AUTOUPDATE"] == "1"
    for bad in ({**pins, "OPENCODE_CONFIG": "/x"}, {**pins, "OPENCODE_CONFIG_CONTENT": "[]"},
                {**pins, "OPENCODE_DISABLE_AUTOUPDATE": "0"}, {"CODEX_PATH": "/bin/true", "NO_BROWSER": "1"}):
        with pytest.raises(PortableError):
            environment.build_process_env(record, {"PATH": "/usr/bin"}, environ=environ, pinned_engine_vars=bad)
    codex = make_connection_record(new_connection_id(), ("/usr/bin/true", "x"), environ=environ)
    with pytest.raises(PortableError):
        environment.build_process_env(codex, {"PATH": "/usr/bin"}, environ=environ, pinned_engine_vars=pins)


# --- install trust ------------------------------------------------------------

def test_install_trust_refuses_links_shared_writes_and_unqualified_versions(tmp_path):
    binary = _fake_binary(tmp_path)
    install = opencode_install.OpenCodeInstall(binary=str(binary), version="2.0.19")
    assert opencode_install.verify_install(install) is install
    assert opencode_install.launch_command_for(install) == (str(binary), "acp")
    link = tmp_path / "link"
    link.symlink_to(binary)
    with pytest.raises(PortableError) as refused:
        opencode_install.trusted_binary(str(link))
    assert refused.value.code == "engine_install_untrusted_location"
    binary.chmod(0o775)
    with pytest.raises(PortableError) as refused:
        opencode_install.trusted_binary(str(binary))
    assert refused.value.code == "engine_install_untrusted_location"
    binary.chmod(0o644)
    with pytest.raises(PortableError):
        opencode_install.trusted_binary(str(binary))
    binary.chmod(0o755)
    with pytest.raises(PortableError) as refused:
        opencode_install.verify_install(opencode_install.OpenCodeInstall(binary=str(binary), version="2.0.18"))
    assert refused.value.code == "opencode_version_unqualified"


@needs_bwrap
def test_version_is_read_in_a_sandbox_and_discovery_reports_it(tmp_path):
    binary = _fake_binary(tmp_path)
    assert opencode_install.probe_version(str(binary)) == "2.0.19"
    found = discovery.discover_opencode(binary=str(binary), environ={"HOME": str(tmp_path), "PATH": "/nonexistent"})
    assert found.launch_command == (str(binary), "acp") and found.version == "2.0.19"
    assert found.native_credential_dir == str(tmp_path / ".local" / "share" / "opencode")
    install = opencode_install.build_install(str(binary))
    assert install.version == "2.0.19"
    silent = tmp_path / "silent"
    silent.write_text("#!/bin/sh\nexit 0\n")
    silent.chmod(0o755)
    assert opencode_install.probe_version(str(silent)) is None
    with pytest.raises(PortableError):
        opencode_install.build_install(str(silent))


# --- launch refusals ----------------------------------------------------------

def test_opencode_never_launches_unjailed_or_unverified(tmp_path):
    environ = isolated_environ(tmp_path)
    binary = _fake_binary(tmp_path)
    record = make_connection_record(new_connection_id(), (str(binary), "acp"), engine="opencode", environ=environ)
    install = opencode_install.OpenCodeInstall(binary=str(binary), version="2.0.19")
    with pytest.raises(PortableError) as refused:
        EngineProcess(record, str(tmp_path), environ=environ, install=install)
    assert refused.value.code == "opencode_requires_engine_jail"
    with pytest.raises(PortableError) as refused:
        EngineProcess(record, str(tmp_path), environ=environ, jail={"scopes": []})
    assert refused.value.code == "opencode_install_unverified"


def test_setuid_bubblewrap_is_refused(tmp_path, monkeypatch):
    from cortex_runtime.engine import process as process_module

    fake = tmp_path / "bwrap"
    fake.write_text("#!/bin/sh\n")
    fake.chmod(0o755 | stat.S_ISUID)
    if not fake.stat().st_mode & stat.S_ISUID:
        pytest.skip("this filesystem drops the setuid bit")
    monkeypatch.setattr(process_module.shutil, "which", lambda name, *a, **k: str(fake) if name == "bwrap" else None)
    with pytest.raises(PortableError) as refused:
        process_module._require_opencode_isolation({"scopes": []}, object())
    assert refused.value.code == "opencode_process_isolation_unavailable"


# --- engine jail options -----------------------------------------------------

@needs_bwrap
def test_isolated_jail_hides_masked_config_and_mounts_the_prompt_read_only(tmp_path):
    project = tmp_path / "project"
    (project / "src").mkdir(parents=True)
    home = tmp_path / "home"
    (home / ".config" / "opencode").mkdir(parents=True)
    (home / ".config" / "opencode" / "opencode.json").write_text("{}")
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    prompt = scratch / "cortex-opencode-agent.md"
    script = f"""
        ls -A {home}/.config/opencode | wc -l
        cat {prompt}; echo
        echo tamper > {prompt} || echo prompt-ro
        ls /proc | grep -c '^[0-9]*$'
    """
    with engine_jail(project, scratch, ["sh", "-c", script], ["src/**"], env={"HOME": str(home), "PATH": "/usr/bin:/bin"},
                     isolate_processes=True, masked=[str(home / ".config" / "opencode")],
                     data_files={str(prompt): b"PROMPT-TEXT"}) as (command, fds):
        assert command[command.index("--die-with-parent") + 1:][:3] == ["--unshare-user", "--unshare-pid", "--unshare-ipc"]
        result = subprocess.run(command, pass_fds=fds, env={"PATH": "/usr/bin:/bin"}, capture_output=True,
                                text=True, timeout=60)
    lines = result.stdout.split()
    assert lines[:3] == ["0", "PROMPT-TEXT", "prompt-ro"], result.stderr
    # Its own PID namespace: only the jail's own few processes are visible.
    assert int(lines[3]) < 10
    # The host copy of the prompt is only the empty mount point.
    assert prompt.read_text() == ""
    assert (home / ".config" / "opencode" / "opencode.json").read_text() == "{}"


@needs_bwrap
def test_a_sibling_jail_cannot_read_an_isolated_jails_environment(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    home = tmp_path / "home"
    home.mkdir()
    scratch_a, scratch_b = tmp_path / "a", tmp_path / "b"
    scratch_a.mkdir()
    scratch_b.mkdir()
    marker = f"cortex-isolation-{os.getpid()}"
    env = {"HOME": str(home), "PATH": "/usr/bin:/bin", "ISOLATION_CHECK": "1"}
    with engine_jail(project, scratch_a, ["sh", "-c", "sleep 30; true", marker], [], env=env,
                     isolate_processes=True) as (command, fds):
        target = subprocess.Popen(command, pass_fds=fds, env={"PATH": "/usr/bin:/bin"})
    try:
        pid = None
        for _ in range(100):
            for name in os.listdir("/proc"):
                if name.isdigit():
                    try:
                        argv = Path(f"/proc/{name}/cmdline").read_bytes().split(b"\0")
                    except OSError:
                        continue
                    if argv[:1] == [b"sh"] and marker.encode() in argv:
                        pid = name
            if pid:
                break
            time.sleep(0.05)
        assert pid, "the jailed process was not found"
        probe = f"import sys\ntry:\n open('/proc/{pid}/environ','rb').close();print('readable')\nexcept OSError: print('denied')"
        with engine_jail(project, scratch_b, [sys.executable, "-c", probe], [], env={"HOME": str(home), "PATH": "/usr/bin:/bin"}) as (c2, f2):
            sibling = subprocess.run(c2, pass_fds=f2, env={"PATH": "/usr/bin:/bin"}, capture_output=True, text=True,
                                     timeout=60)
        assert sibling.stdout.strip() == "denied", sibling.stderr
    finally:
        target.terminate()
        target.wait(10)


# --- refusals -----------------------------------------------------------------

def test_a_refusal_is_recognised_from_the_requests_own_reject_options():
    from cortex_runtime.engine.session_manager import _is_refusal

    params = {"options": [{"optionId": "once", "kind": "allow_once"}, {"optionId": "reject", "kind": "reject_once"}]}
    assert _is_refusal({"outcome": {"outcome": "selected", "optionId": "reject"}}, params)
    assert _is_refusal({"outcome": {"outcome": "cancelled"}}, params)
    assert not _is_refusal({"outcome": {"outcome": "selected", "optionId": "once"}}, params)
    assert not _is_refusal({}, params)


class _RefusedTurnManager:
    """Ends each turn the way OpenCode 2.0.19 does after a refused tool call."""

    def __init__(self, refusals):
        self.prompts = []
        self.refusals = refusals

    def prompt_turn(self, session_ref, message, *, prompt_blocks=None):
        from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind, StopReason

        self.prompts.append(message)
        refused = len(self.prompts) <= self.refusals
        yield EngineEvent(kind=EngineEventKind.TURN_DELTA, delta_text="refused" if refused else "done")
        yield EngineEvent(kind=EngineEventKind.TURN_END,
                          stop_reason=StopReason.CANCELLED if refused else StopReason.END_TURN)

    def take_permission_refusal(self, session_ref):
        return len(self.prompts) <= self.refusals

    def cancel(self, session_ref):
        pass


def _adapter(manager):
    import threading

    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._lock = threading.Lock()
    adapter._store = None
    adapter._session_to_attempt = {"sess_1": "attempt_1"}
    adapter._attempt_task = {"attempt_1": "task_1"}
    adapter._managers = {"attempt_1": manager}
    return adapter


def test_a_refused_tool_call_continues_the_same_session_instead_of_cancelling_it():
    import threading

    from cortex_runtime.portable.engine_contract import StopReason

    manager = _RefusedTurnManager(refusals=1)
    adapter = _adapter(manager)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert manager.prompts == ["do the thing", adapter._REFUSAL_CONTINUE_PROMPT]
    assert outcome.stop_reason == StopReason.END_TURN


def test_repeated_refusals_stop_continuing_and_stay_cancelled():
    import threading

    from cortex_runtime.portable.engine_contract import StopReason

    manager = _RefusedTurnManager(refusals=10)
    adapter = _adapter(manager)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert len(manager.prompts) == 1 + adapter._MAX_REFUSAL_CONTINUATIONS
    assert outcome.stop_reason == StopReason.CANCELLED


def test_a_user_cancellation_is_never_continued():
    import threading

    from cortex_runtime.portable.engine_contract import StopReason

    manager = _RefusedTurnManager(refusals=1)
    cancel = threading.Event()
    cancel.set()
    outcome = _adapter(manager).prompt("sess_1", "do the thing", cancel)
    assert manager.prompts == ["do the thing"]
    assert outcome.stop_reason == StopReason.CANCELLED


# --- routing ------------------------------------------------------------------

def test_automatic_routing_never_picks_opencode_unless_opted_in():
    from cortex_runtime.routing.auto_route import _default_auto_denied

    entry = type("Entry", (), {"route": ExecutionRoute(engine="opencode", connection_id=new_connection_id(),
                                                       model="anthropic/claude-sonnet-4-5")})()
    assert _default_auto_denied({}, entry)
    assert not _default_auto_denied({"auto_allow": ["opencode/anthropic/claude-sonnet-4-5"]}, entry)
    codex = type("Entry", (), {"route": ExecutionRoute(engine="codex-acp", connection_id=new_connection_id(),
                                                       model="gpt-6-sol")})()
    assert not _default_auto_denied({}, codex)


# --- onboarding ---------------------------------------------------------------

@needs_bwrap
def test_add_opencode_connection_references_the_owners_login(tmp_path):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
    import cortex_acp_setup

    environ = isolated_environ(tmp_path)
    binary = _fake_binary(tmp_path)
    missing = cortex_acp_setup.add_opencode_connection(binary=str(binary), environ=environ)
    assert missing["state"] == "authorization_required" and missing["next_steps"][0] == "opencode auth login"
    data = Path(environ["HOME"]) / ".local" / "share" / "opencode"
    data.mkdir(parents=True)
    data.chmod(0o775)
    (data / "auth.json").write_text("{}")
    loose = cortex_acp_setup.add_opencode_connection(binary=str(binary), environ=environ)
    assert loose["connection_id"] == missing["connection_id"]
    assert loose["state"] == "invalid" and loose["next_steps"][0].startswith("chmod 700")
    data.chmod(0o700)
    ready = cortex_acp_setup.add_opencode_connection(binary=str(binary), environ=environ)
    assert ready["state"] == "configured" and ready["launch_command"] == [str(binary), "acp"]
    assert ready["credential_dir_kind"] == "referenced" and ready["version"] == "2.0.19"
    old = _fake_binary(tmp_path / "old", version="2.0.18")
    with pytest.raises(SystemExit) as refused:
        cortex_acp_setup.add_opencode_connection(binary=str(old), environ=environ)
    assert "opencode_version_unqualified" in str(refused.value)
