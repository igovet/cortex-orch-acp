#!/usr/bin/env python3
"""Standalone fake downstream ACP engine (test-only; see engine_fakes.py).

No import of ``cortex_runtime`` or ``tests`` -- this runs as a real
subprocess launched from a ``ConnectionRecord.launch_command``. It never
claims to emulate a real engine (codex-acp/claude-agent-acp); it is a
scripted collaborator for T012's process/session/client/correlation tests.

Usage: ``python3 -B fake_engine_process.py <scenario.json>``
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time

STDOUT = sys.stdout.buffer
STDIN = sys.stdin.buffer
_WRITE_LOCK = threading.Lock()


def _write_raw(data: bytes) -> None:
    with _WRITE_LOCK:
        try:
            STDOUT.write(data)
            STDOUT.flush()
        except (BrokenPipeError, OSError):
            pass


def _write_json(obj) -> None:
    _write_raw((json.dumps(obj, ensure_ascii=False) + "\n").encode("utf-8"))


def _write_malformed(mode: str) -> None:
    if mode == "non_json":
        _write_raw(b"not valid json at all\n")
    elif mode == "batch":
        _write_raw(b"[]\n")
    elif mode == "oversize":
        _write_raw(b"{" + b"\"x\":\"" + (b"a" * 3_000_000) + b"\"}\n")
    elif mode == "nesting_bomb":
        _write_raw((b"[" * 100_000) + (b"]" * 100_000) + b"\n")
    elif mode == "invalid_utf8":
        _write_raw(b"\xff\xfe not utf8\n")


def _notify(method, params) -> None:
    _write_json(dict(jsonrpc="2.0", method=method, params=params))


def _result(request_id, result) -> None:
    _write_json(dict(jsonrpc="2.0", id=request_id, result=result))


def _error(request_id, code, message) -> None:
    _write_json(dict(jsonrpc="2.0", id=request_id, error=dict(code=code, message=message)))


class FakeEngine:
    def __init__(self, scenario: dict):
        self.scenario = scenario
        self._cancel_events: dict[str, threading.Event] = {}
        self._prompt_counts: dict[str, int] = {}
        self._session_params = {}
        self._client_request_id = 1_000_000
        # T014: pending requests *this* fake engine issued to the client
        # (session/request_permission, fs/*, terminal/*), keyed by the id
        # it chose, so replies can be correlated and (optionally) logged.
        self._client_pending: dict[object, dict] = {}
        self._client_pending_lock = threading.Lock()

    def run(self) -> None:
        env_dump_path = self.scenario.get("env_dump_path")
        if env_dump_path:
            try:
                with open(env_dump_path, "w", encoding="utf-8") as handle:
                    json.dump(sorted(os.environ.keys()), handle)
            except OSError:
                pass
        env_dump_values_path = self.scenario.get("env_dump_values_path")
        if env_dump_values_path:
            try:
                with open(env_dump_values_path, "w", encoding="utf-8") as handle:
                    json.dump(dict(os.environ), handle)
            except OSError:
                pass
        if self.scenario.get("ignore_sigterm"):
            # Only SIGTERM is ignored; SIGKILL (untrappable) still works,
            # exercising EngineProcess.terminate()'s SIGTERM -> grace ->
            # SIGKILL escalation.
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
        grandchild_pid_file = self.scenario.get("spawn_grandchild_pid_file")
        if grandchild_pid_file:
            # A grandchild left in the *same* process group as this fake
            # engine (no start_new_session here) -- EngineProcess.terminate()
            # signals the whole group, so it must die too.
            grandchild = subprocess.Popen(["sleep", "300"])
            with open(grandchild_pid_file, "w", encoding="utf-8") as handle:
                handle.write(str(grandchild.pid))
        stderr_flood_bytes = self.scenario.get("stderr_flood_bytes")
        if stderr_flood_bytes:
            threading.Thread(target=self._flood_stderr, args=(stderr_flood_bytes,), daemon=True).start()
        for raw_line in iter(STDIN.readline, b""):
            line = raw_line.strip(b"\n")
            if not line:
                continue
            try:
                message = json.loads(line.decode("utf-8"))
            except (UnicodeDecodeError, ValueError):
                continue
            if not isinstance(message, dict):
                continue
            method = message.get("method")
            request_id = message.get("id")
            if method is None and "id" in message and ("result" in message or "error" in message):
                self._handle_client_response(message)
                continue
            request_log_path = self.scenario.get("request_log_path")
            if request_log_path and isinstance(method, str):
                try:
                    with open(request_log_path, "a", encoding="utf-8") as handle:
                        # T027/EXT-01 (mixed-team route resolution): also
                        # log this request's own ``params`` verbatim --
                        # additive next to the pre-existing ``method``/
                        # ``has_id`` fields every prior consumer of this log
                        # already reads, so a test can assert the actual
                        # wire-level model/effort/mode a
                        # ``session/set_config_option`` (or any other) call
                        # sent, not only the DB audit row a production
                        # caller separately records for the same attempt.
                        handle.write(json.dumps(dict(
                            method=method, has_id="id" in message, params=message.get("params"),
                        )) + "\n")
                except OSError:
                    pass
            if method == "initialize":
                self._handle_initialize(request_id)
            elif method == "session/new":
                self._handle_session_new(request_id, message.get("params") or {})
            elif method == "session/set_config_option":
                self._handle_set_config_option(request_id, message.get("params") or {})
            elif method == "session/prompt":
                threading.Thread(
                    target=self._handle_prompt, args=(request_id, message.get("params") or {}), daemon=True,
                ).start()
            elif method == "session/cancel":
                self._handle_cancel(message.get("params") or {})
            # Unknown methods/notifications are silently ignored (mirrors a
            # real engine's own "ignore what it doesn't recognize" duty).
        if self.scenario.get("ignore_sigterm"):
            # stdin EOF (the parent closed its write end as the first step
            # of EngineProcess.terminate()) would otherwise let this loop
            # -- and the process -- exit gracefully on its own; a fake
            # engine truly modeling "ignores SIGTERM" must also outlive
            # that, so the only way the parent can actually end it is
            # SIGKILL.
            time.sleep(300)

    def _handle_client_response(self, message) -> None:
        request_id = message.get("id")
        log_path = self.scenario.get("client_reply_log_path")
        if log_path:
            try:
                with open(log_path, "a", encoding="utf-8") as handle:
                    handle.write(json.dumps(message) + "\n")
            except OSError:
                pass
        with self._client_pending_lock:
            entry = self._client_pending.get(request_id)
        if entry is not None:
            entry["response"] = message
            entry["event"].set()

    def _send_client_request(self, request, *, wait_timeout=5.0):
        """Send one agent->client request (T014: permission/fs/terminal),
        wait up to ``wait_timeout`` for its reply, and return it (or
        ``None`` on timeout). ``request`` may set an explicit ``id`` (to
        exercise a duplicate-id scenario) or omit it for an auto id."""
        request_id = request.get("id")
        if request_id is None:
            self._client_request_id += 1
            request_id = self._client_request_id
        event = threading.Event()
        with self._client_pending_lock:
            self._client_pending[request_id] = dict(event=event, response=None)
        _write_json(dict(jsonrpc="2.0", id=request_id, method=request["method"], params=request.get("params") or {}))
        event.wait(wait_timeout)
        with self._client_pending_lock:
            entry = self._client_pending.pop(request_id, None)
        return entry["response"] if entry is not None else None

    def _handle_initialize(self, request_id) -> None:
        malformed = self.scenario.get("malformed_mode")
        if malformed:
            _write_malformed(malformed)
        if self.scenario.get("send_unsolicited_response_first"):
            # A response whose id was never sent by the client -- must be
            # recognized as unknown and dropped, never matched to any
            # pending call.
            _result(999_999_999, dict(configOptions=[]))
        caps = self.scenario.get("agent_capabilities") or {}
        _result(request_id, dict(agentCapabilities=caps))

    def _next_session_id(self) -> str:
        """``session_ids`` (a list, T014 multi-session scenarios) is
        consumed one id per ``session/new`` call, in order; falls back to
        the single fixed ``session_id`` once exhausted (or if unset), same
        as every pre-T014 scenario."""
        session_ids = self.scenario.get("session_ids")
        if session_ids:
            with self._client_pending_lock:
                index = self.scenario.setdefault("_session_ids_consumed", 0)
                if index < len(session_ids):
                    self.scenario["_session_ids_consumed"] = index + 1
                    return session_ids[index]
        return self.scenario["session_id"]

    def _handle_session_new(self, request_id, params) -> None:
        # T026 (additive, opt-in, default None -- every pre-T026 scenario is
        # unaffected): dumps this exact session/new call's own full params
        # (including any "_meta") to a scenario-named file, so a test can
        # assert on the real wire-level payload a qualified adapter-specific
        # delivery channel (e.g. claude-agent-acp's session/new
        # _meta.systemPrompt) actually sent -- never only asserting the
        # in-process DeliveryPlan/registry classification.
        dump_path = self.scenario.get("session_new_params_dump_path")
        if dump_path:
            with open(dump_path, "w", encoding="utf-8") as handle:
                json.dump(params, handle)
        session_id = self._next_session_id()
        self._session_params[session_id] = params
        for update in self.scenario.get("replay_updates") or []:
            _notify("session/update", dict(sessionId=session_id, update=update))
        payload = dict(sessionId=session_id)
        options = self.scenario.get("config_options")
        if options:
            payload["configOptions"] = options
        _result(request_id, payload)

    def _handle_set_config_option(self, request_id, params) -> None:
        config_id = params.get("configId")
        # "configure_behavior_by_option" (opt-in, defaults to {}) lets a
        # scenario script a *different* behavior per configId, layered over
        # the scenario-wide "configure_behavior" default -- needed to
        # exercise a partial multi-option configure() (e.g. the first
        # option accepted, a later one rejected) without changing any
        # existing scenario's uniform behavior.
        behavior = (self.scenario.get("configure_behavior_by_option") or {}).get(
            config_id, self.scenario.get("configure_behavior", "accept")
        )
        if behavior == "hang":
            return
        if behavior == "error":
            _error(request_id, -32001, "engine_rejected_configuration")
            return
        if behavior == "accept_no_config_options":
            # A "successful" (no JSON-RPC error) reply that omits
            # configOptions entirely -- no evidence either way of what was
            # actually applied.
            _result(request_id, {})
            return
        options = list(self.scenario.get("config_options") or [])
        value = params.get("value")
        updated = []
        for option in options:
            option = dict(option)
            if option.get("id") == config_id:
                option["currentValue"] = value if behavior == "accept" else "unchanged-mismatch-value"
            updated.append(option)
        # "remove_options_on_apply" (opt-in, defaults to {}): once
        # ``config_id`` is accepted, drop the named option id(s) from every
        # subsequent (and this) configOptions response -- models a
        # dynamically-narrowing option set (e.g. codex-acp's reasoning
        # effort choices depending on the selected model) so a caller can
        # exercise "an earlier-observed option is no longer offered after
        # an application earlier in the same configure() call".
        remove_map = self.scenario.get("remove_options_on_apply") or {}
        if behavior == "accept" and config_id in remove_map:
            to_remove = set(remove_map[config_id])
            updated = [option for option in updated if option.get("id") not in to_remove]
        self.scenario["config_options"] = updated
        _result(request_id, dict(configOptions=updated))

    def _handle_cancel(self, params) -> None:
        session_id = params.get("sessionId")
        event = self._cancel_events.setdefault(session_id, threading.Event())
        event.set()

    def _flood_stderr(self, total_bytes: int) -> None:
        chunk = (b"x" * 4096) + b"\n"
        written = 0
        while written < total_bytes:
            try:
                sys.stderr.buffer.write(chunk)
                sys.stderr.buffer.flush()
            except (BrokenPipeError, OSError, ValueError):
                return
            written += len(chunk)

    def _runtime_gateway_prompt(self, session_id, params):
        """Scripted proposals over the real runtime MCP, never a model API."""
        import select
        server=next(s for s in self._session_params[session_id]['mcpServers'] if s['name']=='cortex')
        env=dict(os.environ,**{item['name']:item['value'] for item in server.get('env',[])})
        child=subprocess.Popen([server['command'],*server.get('args',[])],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,bufsize=0)
        sequence=0;buffer=b''
        def rpc(method,arguments):
            nonlocal sequence,buffer
            sequence+=1
            child.stdin.write((json.dumps(dict(jsonrpc='2.0',id=sequence,method=method,params=arguments))+'\n').encode());child.stdin.flush()
            deadline=time.monotonic()+15
            while True:
                while b'\n' not in buffer:
                    left=deadline-time.monotonic()
                    if left<=0 or not select.select([child.stdout],[],[],left)[0]:raise RuntimeError('MCP timeout')
                    chunk=os.read(child.stdout.fileno(),65536)
                    if not chunk:raise RuntimeError('MCP closed')
                    buffer+=chunk
                line,buffer=buffer.split(b'\n',1);reply=json.loads(line)
                if reply.get('id')==sequence:return reply
        def call(name,**args):
            tool_id='scripted:'+str(sequence+1)
            _notify('session/update',dict(sessionId=session_id,update=dict(sessionUpdate='tool_call',toolCallId=tool_id,title='mcp__cortex__'+name,kind='other',status='in_progress',rawInput=args)))
            reply=rpc('tools/call',dict(name=name,arguments=args))
            result=reply.get('result',{})
            value=json.loads(result['content'][0]['text']) if result.get('content') else reply
            _notify('session/update',dict(sessionId=session_id,update=dict(sessionUpdate='tool_call_update',toolCallId=tool_id,status='failed' if result.get('isError') or 'error' in reply else 'completed',rawOutput=value)))
            return value
        try:
            rpc('initialize',dict(protocolVersion='2025-11-25',capabilities={},clientInfo=dict(name='scripted-test',version='1')))
            child.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n');child.stdin.flush()
            listed=rpc('tools/list',{})
            names={tool['name'] for tool in listed['result']['tools']}
            if 'plan_submit' in names:
                context=call('task_get_context')
                if context['state']=='PLANNING':
                    value=call('plan_submit',plan=dict(tasks=[dict(id='inspect',role='explorer',objective='Inspect the project',dependencies=[],scope=['src/**'],model='auto')],verification=[['python3','-B','-c','print("verified fixture")']]),request_key='plan-'+str(context['revision']))
                else:value=call('synthesis_submit',text='Report independently reviewed and runtime verification passed.')
            else:
                draft=call('create_draft',template='general',request_key='draft')
                call('draft_write',draft_id=draft['draft_id'],content=draft['required_first_line']+'\n\n# Report\n\nObserved disposable fixture.\n',request_key='draft-body')
                report=call('write_report',draft_id=draft['draft_id'],title='Observed report',summary='Observed fixture',author='worker',request_key='report')
                result_args=dict(report=dict(report_ref=report['report_id']),summary='Observed fixture',checks=[],request_key='result')
                value=call('result_submit',**result_args)
                if value.get('error')=='review_verdict_required':value=call('result_submit',**dict(result_args,verdict='APPROVE'))
            if value.get('error'):raise RuntimeError(json.dumps(value))
            _notify('session/update',dict(sessionId=session_id,update=dict(sessionUpdate='agent_message_chunk',content=dict(type='text',text='Visible scripted peer response.'))))
        finally:
            child.stdin.close()
            try:child.wait(timeout=3)
            except subprocess.TimeoutExpired:child.kill();child.wait()

    def _handle_prompt(self, request_id, params) -> None:
        session_id = params.get("sessionId")
        prompt_number = self._prompt_counts.get(session_id, 0)
        self._prompt_counts[session_id] = prompt_number + 1
        prompt_log_path = self.scenario.get("prompt_log_path")
        if prompt_log_path:
            with open(prompt_log_path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(dict(session_id=session_id, prompt_number=prompt_number,
                                             prompt=params.get("prompt")), ensure_ascii=False) + "\n")
        # Cancellation is scoped to one turn, never sticky across a later
        # turn on the same session -- reset at the start of every prompt
        # (matches real ACP semantics: a fresh session/prompt call starts a
        # fresh cancellable turn).
        cancel_event = self._cancel_events.setdefault(session_id, threading.Event())
        cancel_event.clear()
        behavior = (self.scenario.get("prompt_behavior_by_session") or {}).get(
            session_id, self.scenario.get("prompt_behavior", "normal")
        )

        if behavior == 'runtime_gateway':
            try:
                self._runtime_gateway_prompt(session_id,params)
                _result(request_id,dict(stopReason='end_turn'))
            except Exception as error:
                _error(request_id,-32603,'Scripted MCP peer failed: '+str(error))
            return

        if behavior == "error":
            # A JSON-RPC error reply to session/prompt, with the scenario's
            # own code/message/data -- e.g. codex-acp 1.13.0's refusal of a
            # turn on an exhausted plan: -32603 with
            # data.codexErrorInfo="usageLimitExceeded".
            scripted = self.scenario.get("prompt_error") or {}
            _write_json(dict(jsonrpc="2.0", id=request_id, error=dict(
                code=scripted.get("code", -32603), message=scripted.get("message", "Internal error"),
                data=scripted.get("data"))))
            return
        if behavior == "hang":
            return  # never reply, never emit -- caller times out
        if behavior == "silent_until_cancel":
            # T035 FINDING-06: models an engine that is genuinely silent --
            # no session/update notifications at all -- for as long as the
            # caller lets it run, and is unstuck *only* by session/cancel
            # (never by any clock). Used to prove cancellation is a real,
            # prompt way out of an otherwise-unbounded prompt turn now that
            # session/prompt itself carries no timeout.
            cancel_event.wait()
            _result(request_id, dict(stopReason="cancelled"))
            return
        if behavior == "one_update_then_hang":
            # Emit exactly the first scripted update, then behave exactly
            # like "hang": never reply, never emit again -- lets a test
            # drive the host-side generator to a real suspended yield
            # point before abandoning it (unlike a bare "hang", where the
            # generator would never have started at all).
            updates = self.scenario.get("prompt_updates") or []
            if updates:
                _notify("session/update", dict(sessionId=session_id, update=updates[0]))
            return
        if behavior == "crash":
            for update in self.scenario.get("prompt_updates") or []:
                _notify("session/update", dict(sessionId=session_id, update=update))
            STDOUT.flush()
            os._exit(7)
        if behavior == "eof_no_exit":
            for update in self.scenario.get("prompt_updates") or []:
                _notify("session/update", dict(sessionId=session_id, update=update))
            try:
                STDOUT.close()
            except OSError:
                pass
            time.sleep(30)
            return

        honor_cancel = behavior != "ignore_cancel"
        updates = self.scenario.get("prompt_updates") or []
        if behavior == "updates_then_wait_for_cancel_once" and prompt_number > 0:
            updates = []
        for update in updates:
            if honor_cancel and cancel_event.is_set():
                break
            delay = update.get("delay", 0.0)
            if delay:
                time.sleep(delay)
            _notify("session/update", dict(sessionId=session_id, update=update))
            if behavior == "malformed_then_normal":
                _write_malformed(self.scenario.get("malformed_mode") or "non_json")

        if behavior == "updates_then_wait_for_cancel_once" and prompt_number == 0:
            cancel_event.wait()
            _result(request_id, dict(stopReason="cancelled"))
            return

        if behavior == "client_request":
            request = self.scenario.get("client_request") or dict(method="request_permission", params={})
            self._client_request_id += 1
            _write_json(dict(jsonrpc="2.0", id=self._client_request_id, method=request["method"], params=request.get("params") or {}))
        elif behavior in {"client_requests_sequence", "client_requests_sequence_skip_role_refusal"}:
            # T014: send each scripted agent->client request in turn,
            # waiting for (and logging, if client_reply_log_path is set)
            # each reply before sending the next. The role-refusal variant
            # lets the coordinator react to Cortex's new instruction without
            # replaying the scripted bad action, while fresh worker sessions
            # still exercise their own permission requests.
            requests = self.scenario.get("client_requests") or []
            prompt_blocks = params.get("prompt") or []
            prompt_text = " ".join(
                block.get("text", "") for block in prompt_blocks
                if isinstance(block, dict) and isinstance(block.get("text"), str)
            )
            if behavior == "client_requests_sequence_skip_role_refusal" and "Cortex role restriction" in prompt_text:
                requests = []
            for request in requests:
                self._send_client_request(request, wait_timeout=request.get("wait_timeout", 5.0))
        elif behavior == "client_requests_fire":
            # T014 review: send every scripted request back-to-back with no
            # wait in between (to exercise a client-side concurrency cap),
            # then wait (bounded) for replies to arrive so
            # client_reply_log_path captures them before this turn ends.
            pending_ids = []
            for request in self.scenario.get("client_requests") or []:
                client_request_id = request.get("id")
                if client_request_id is None:
                    self._client_request_id += 1
                    client_request_id = self._client_request_id
                with self._client_pending_lock:
                    self._client_pending[client_request_id] = dict(event=threading.Event(), response=None)
                pending_ids.append(client_request_id)
                _write_json(dict(jsonrpc="2.0", id=client_request_id, method=request["method"], params=request.get("params") or {}))
            deadline = time.time() + self.scenario.get("client_requests_fire_wait", 3.0)
            while time.time() < deadline:
                with self._client_pending_lock:
                    if not any(rid in self._client_pending for rid in pending_ids):
                        break
                time.sleep(0.02)

        stop_reason = "cancelled" if (honor_cancel and cancel_event.is_set()) else self.scenario.get("prompt_stop_reason", "end_turn")

        if behavior == "late_reply":
            time.sleep(self.scenario.get("late_reply_delay", 1.5))

        response = dict(stopReason=stop_reason)
        if self.scenario.get("prompt_usage") is not None:
            response["usage"] = self.scenario["prompt_usage"]
        _result(request_id, response)


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: fake_engine_process.py <scenario.json>", file=sys.stderr)
        return 2
    with open(sys.argv[1], "r", encoding="utf-8") as handle:
        scenario = json.load(handle)
    FakeEngine(scenario).run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
