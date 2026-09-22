"""Small newline-delimited JSON-RPC MCP server with value-free diagnostics.

Every ``tools/call`` reaches ``Store.call`` through exactly one identity
path: the trusted channel (``_dispatch_channeled``, which verifies a channel
secret and always passes ``trusted=True``). The Codex-plugin-harness-era
unchanneled ``_meta`` identity path (native Codex CLI turn metadata) was
removed at T033; every server process now requires channel credentials.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time
import traceback
from collections import OrderedDict

from .contracts import BY_NAME, MAX_REQUEST_BYTES, StoreError, TOOLS, ERROR_HELP, validate, clip_summary_preview
from .channeled_tools import channeled_base_tools
from .delegation import contracts as delegation_contracts
from .delegation import mcp_tools as delegation_mcp_tools
from .store import Store, private_directory
from .project_storage import migrate_legacy_project_storage, project_store_directory, canonical_project
from .trusted_channel import resolve_channel
from .ownership import current_owner_epoch
from .diagnostic_boundary import emit_bounded_diagnostic, run_passive_diagnostic

PLUGIN = Path(__file__).resolve().parents[2]
VERSION = json.loads((PLUGIN / 'package-identity.json').read_text())['version']
CATALOGUE_DIGEST = hashlib.sha256(json.dumps(TOOLS, sort_keys=True).encode()).hexdigest()

# Tools that mutate task-owned state and therefore require the caller's
# trusted channel to carry the task's current ownership epoch. Reads
# (list_reports, read_report, read_draft) never require ownership.
MUTATING_TOOLS = frozenset({'create_draft', 'write_report', 'set_governance'})

# Reads expose only the bound task; project_read additionally requires a live
# assignment/epoch because it reads the current project through the gateway.
DELEGATION_READ_OPERATIONS = frozenset({
    'list_sources','read_source','read_capsule','read_large_output',
    'read_diagnostics','read_skill','task_get_context','project_read',
})


def safe_observation_fields(operation,arguments,result=None):
    """Expose only bounded routing selectors needed to audit agent behavior."""
    arguments=arguments if isinstance(arguments,dict) else {}
    result=result if isinstance(result,dict) else {}
    capture=result.get('source_capture',{})
    common=dict(source_capture_status=capture.get('status'),
                source_revision=capture.get('revision'))
    if operation=='create_task':
        return common|dict(original_request_sha256=result.get('original_request_sha256'))
    if operation=='read_report':
        report_id=result.get('report_id') or arguments.get('report_id')
        return common|dict(document_kind=result.get('kind') or ('pipeline' if not report_id else 'unresolved'),
                           report_id=report_id,
                           requested_limit=arguments.get('limit',4000),
                           page='continuation' if arguments.get('cursor') else 'start')
    if operation=='list_reports':
        return common|dict(requested_limit=arguments.get('limit',25),
                           page='continuation' if arguments.get('cursor') else 'start')
    if operation=='read_draft':return common|dict(
        draft_id=arguments.get('draft_id'),page='continuation' if arguments.get('cursor') else 'start')
    if operation=='create_draft':return common|dict(template=arguments.get('template'),
                                                   draft_id=result.get('draft_id'))
    if operation=='write_report':
        return common|dict(draft_id=arguments.get('draft_id'),report_id=result.get('report_id'),
                           summary_characters=len(arguments.get('summary','')))
    if operation=='set_governance':return common|dict(governance_mode=arguments.get('mode'))
    return common


def _write_observation(operation, outcome, replayed=False, meta=None, arguments=None, result=None):
    """Write one optional bounded passive observation."""
    location = os.environ.get('CORTEX_OBSERVATION_DIR')
    if not location:
        return
    root = private_directory(location)
    path = root / f'{os.getpid()}.jsonl'
    row = dict(pid=os.getpid(), time_ns=time.time_ns(), operation=operation,
               outcome=outcome, replayed=replayed, version=VERSION,
               catalogue_digest=CATALOGUE_DIGEST)
    if operation == 'initialize':
        row['package_path'] = str(PLUGIN)
    elif operation in BY_NAME:
        row.update(safe_observation_fields(operation,arguments,result))
    fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        if os.fstat(fd).st_size < 256_000:
            os.write(fd, (json.dumps(row)+'\n').encode())
    finally:
        os.close(fd)


def _log_unexpected(context):
    """Write the current traceback to stderr for a swallowed unexpected error.

    The client-facing reply stays deliberately opaque (see the catch-alls
    that call this), but the real cause -- almost always a non-StoreError
    like a raw ``sqlite3.OperationalError: database is locked`` under lock
    contention -- must not vanish: the engine drains this process's stderr
    into its EngineLogs, so this is the "local server" the reply tells the
    operator to check. Never raises (a diagnostic must never alter outcome).
    """
    try:
        sys.stderr.write(f"cortex-server unexpected error [{context}]:\n" + traceback.format_exc())
        sys.stderr.flush()
    except Exception:
        pass


def observe(operation, outcome, replayed=False, meta=None, arguments=None, result=None):
    """Optional bounded passive observation; failures never alter request outcome."""
    return run_passive_diagnostic(
        lambda: _write_observation(operation, outcome, replayed, meta, arguments, result),
        operation="observation",
        emit=lambda finding: emit_bounded_diagnostic(sys.stderr, finding),
    )


def _delegation_operation_names(role_kind):
    try:
        return set(delegation_contracts.schemas_for(role_kind))
    except StoreError:
        return set()


class ProtocolError(Exception):
    def __init__(self, code, message):
        self.code=code;self.message=message


class Server:
    def __init__(self, *, channel_credentials=None):
        self.stores = OrderedDict()
        self.store = None
        # (channel_id, secret, project_root), read once at construction from
        # CORTEX_CHANNEL_ID / CORTEX_CHANNEL_SECRET / CORTEX_CHANNEL_PROJECT_ROOT
        # by main(); never re-read per request, never taken from tool
        # arguments or _meta. One server process serves one trusted channel
        # identity for its whole lifetime.
        self.channel_credentials = channel_credentials
        # Lazily resolved once per process (a server process serves exactly
        # one trusted channel identity for its whole lifetime -- see
        # channel_credentials' own docstring above); cached so tools/list and
        # every subsequent tools/call reuse the same resolved role/context
        # instead of re-querying the store per call.
        self._resolved_channel = None

    def _channel_context(self):
        """Resolve (and cache) this process's own trusted channel, if any.

        Returns ``None`` when this server was not launched with channel
        credentials; ``tools/call`` itself already refuses every call in
        that case (see ``dispatch``'s ``channel_required`` check), so
        callers of this method only ever see ``None`` from ``tools/list``.
        Raises ``StoreError`` (mapped to a JSON-RPC error by callers) if
        channel credentials are present but do not resolve to a live
        channel.
        """
        if self._resolved_channel is not None:
            return self._resolved_channel
        if self.channel_credentials is None:
            return None
        channel_id, secret, project_root = self.channel_credentials
        project = canonical_project(project_root)
        migrate_legacy_project_storage(project)
        directory = project_store_directory(project)
        if not (directory/'cortex.sqlite3').exists():
            raise StoreError('task_not_bound')
        store = self.stores.get(project)
        if store is None:
            store = Store(directory, initialize=False, project_root=project)
            self.stores[project] = store
        self.stores.move_to_end(project)
        while len(self.stores)>16: self.stores.popitem(last=False)
        self.store = store
        self._resolved_channel = resolve_channel(store, channel_id, secret)
        return self._resolved_channel

    def dispatch(self, method, params, request_id=None):
        if method == 'initialize':
            if not isinstance(params,dict) or not isinstance(params.get('protocolVersion'),str) or not isinstance(params.get('capabilities'),dict) or not isinstance(params.get('clientInfo'),dict) or any(not isinstance(params['clientInfo'].get(k),str) for k in ('name','version')):
                raise ProtocolError(-32602,'Invalid initialize parameters. Supply protocolVersion (string), capabilities (object), and clientInfo with name and version strings.')
            observe('initialize','success')
            return dict(protocolVersion='2025-11-25', capabilities=dict(tools=dict(listChanged=False)), serverInfo=dict(name='cortex',version=VERSION), instructions='Cortex provides channel-bound reports, structured proposals and scoped project operations; advertised schemas are authoritative. The runtime owns scheduling, permissions, verification and completion. Published Markdown stays in the project while SQLite retains metadata, relationships, revisions and receipts. Native user source capture reports completeness explicitly, and unavailable new host input does not hide already saved reports.')
        if method == 'ping':
            return {}
        if method == 'tools/list':
            try:
                channel = self._channel_context()
            except StoreError as exc:
                raise ProtocolError(-32001, ERROR_HELP.get(str(exc), ERROR_HELP['storage_error'])) from None
            if channel is None:
                return dict(tools=TOOLS)
            return dict(tools=list(channeled_base_tools()) + delegation_mcp_tools.tool_definitions(channel.role))
        if method != 'tools/call':
            raise ProtocolError(-32601,'Unknown method. Use initialize, ping, tools/list or tools/call; discover tool names through tools/list.')
        if not isinstance(params, dict) or set(params)-{'name','arguments','_meta'} or not isinstance(params.get('name'), str) or ('arguments' in params and not isinstance(params['arguments'],dict)):
            observe('tools/call','invalid_arguments')
            raise ProtocolError(-32602,'Malformed tools/call parameters. Supply name as a string and arguments as an object; only name, arguments and _meta are allowed.')
        name = params['name']
        if isinstance(params.get('arguments'), dict):
            params = dict(params, arguments=clip_summary_preview(name, params['arguments']))
        if self.channel_credentials is None:
            observe('tools/call','channel_required')
            return self.error(StoreError('channel_required'))
        try:
            channel = self._channel_context()
        except StoreError as exc:
            observe('tools/call', str(exc))
            return self.error(exc)
        role_operations = _delegation_operation_names(channel.role)
        if name in role_operations:
            try:
                if name not in DELEGATION_READ_OPERATIONS:
                    with self.store.connection() as db:
                        current = current_owner_epoch(db, channel.task_id)
                    if current != channel.owner_epoch:
                        raise StoreError('stale_ownership_epoch')
                arguments = params.get('arguments', {})
                if not isinstance(arguments, dict):
                    raise StoreError('invalid_arguments','arguments',str(type(arguments).__name__),'JSON object','Send one object matching the advertised inputSchema.')
                data = delegation_mcp_tools.dispatch(self.store, channel, name, arguments)
                observe(name,'success',False,None,arguments,data)
                return dict(content=[dict(type='text',text=json.dumps(data,ensure_ascii=False))], structuredContent=data, isError=False)
            except StoreError as exc:
                observe(name, str(exc), meta=params.get('_meta'), arguments=params.get('arguments'))
                return self.error(exc)
            except Exception:
                # A delegation/gateway op that raised something other than a
                # StoreError (e.g. a raw sqlite3.OperationalError from lock
                # contention) must never escape to the opaque -32603 catch-all:
                # log the real cause and return a structured, retryable
                # storage_error, exactly like the BY_NAME branch below already
                # does. Its request_key dedup makes an exact retry idempotent.
                _log_unexpected(f'tools/call:{name}')
                observe(name, 'storage_error', meta=params.get('_meta'), arguments=params.get('arguments'))
                return self.error(StoreError('storage_error'))
        if name in {'create_task','set_governance'} or (name == 'create_draft' and params.get('arguments',{}).get('template') == 'pipeline'):
            return self.error(StoreError('runtime_owned_operation'))
        if name not in BY_NAME:
            observe('tools/call','unknown_tool')
            raise ProtocolError(-32602,'Unknown tool. Call tools/list and use an advertised name; do not reconstruct retired names.')
        try:
            validate(name,params.get('arguments',{}))
            data=self._dispatch_channeled(name,params)
            if name == 'read_report':
                from .delegation.report_reads import record_report_read
                channel = self._channel_context()
                record_report_read(
                    self.store, task_id=channel.task_id, attempt_id=channel.attempt_id,
                    report_id=data.get('report_id') or params.get('arguments', {}).get('report_id'),
                    request_id=request_id,
                )
            observe(name,'success',data.get('replayed',False),None,params.get('arguments'),data)
            return dict(content=[dict(type='text',text=json.dumps(data,ensure_ascii=False))], structuredContent=data, isError=False)
        except StoreError as exc:
            code = str(exc)
            failure = exc
        except Exception:
            _log_unexpected(f'tools/call:{name}')
            code = 'storage_error'
            failure = StoreError(code)
        observe(name,code,meta=params.get('_meta'),arguments=params.get('arguments'))
        return self.error(failure)

    def _dispatch_channeled(self, name, params):
        """Resolve identity from this process's one trusted channel.

        The channel's project root is fixed for this whole server process
        (see ``__init__``); the channel itself supplies the task and
        principal thread, never a model-supplied selector. Bootstrap tasks
        own creation, so ``create_task`` is rejected on this path.
        """
        channel_id,secret,project_root=self.channel_credentials
        project=canonical_project(project_root)
        migrate_legacy_project_storage(project)
        directory=project_store_directory(project)
        if not (directory/'cortex.sqlite3').exists():
            raise StoreError('task_not_bound')
        store=self.stores.get(project)
        if store is None:
            store=Store(directory,initialize=False,project_root=project)
            self.stores[project]=store
        self.stores.move_to_end(project)
        while len(self.stores)>16:self.stores.popitem(last=False)
        self.store=store
        context=resolve_channel(store,channel_id,secret)
        if name=='create_task':
            raise StoreError('channel_task_creation')
        if name in MUTATING_TOOLS:
            with store.connection() as db:
                current=current_owner_epoch(db,context.task_id)
            if current!=context.owner_epoch:
                raise StoreError('stale_ownership_epoch')
        from .orchestration.gateway import _authority
        arguments=dict(params.get('arguments',{}))
        if name in MUTATING_TOOLS:
            validate(name,arguments)
        if name in MUTATING_TOOLS and isinstance(arguments.get('request_key'),str):
            # Storage receipts are task-scoped. Model request identities are
            # channel-scoped: two workers may both choose "draft" safely.
            import hashlib
            arguments['request_key']=hashlib.sha256(
                (context.channel_id+'\0'+arguments['request_key']).encode()).hexdigest()
        result=store.call(name, arguments,
                           context.principal_thread_id, context.parent_principal_thread_id,
                           original_request=None, steering_source=None, trusted=True,
                           authorization=(lambda db: _authority(db,context)) if name in MUTATING_TOOLS else None)
        return result

    @staticmethod
    def error(failure):
        def public(value):
            if not isinstance(value,str): return value
            return re.sub(r'(?<![0-9a-f])t_[0-9a-f]{12}(?![0-9a-f])','<task>',value)
        code=str(failure)
        data=dict(error=code,message=ERROR_HELP.get(code,ERROR_HELP['storage_error']),
                  correction=public(failure.correction or ERROR_HELP.get(code,ERROR_HELP['storage_error'])))
        if failure.field:data['field']=failure.field
        if failure.received is not None:data['received']=public(failure.received)
        if failure.expected is not None:data['expected']=public(failure.expected)
        data['write_state']='uncertain' if code in ('storage_error','file_conflict','unsafe_storage') else 'rejected'
        return dict(content=[dict(type='text',text=json.dumps(data))],isError=True)


def reject_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError
        result[key] = value
    return result


def main():
    os.umask(0o077)
    channel_id=os.environ.get('CORTEX_CHANNEL_ID')
    channel_secret=os.environ.get('CORTEX_CHANNEL_SECRET')
    channel_project_root=os.environ.get('CORTEX_CHANNEL_PROJECT_ROOT')
    channel_credentials=((channel_id,channel_secret,channel_project_root)
                          if channel_id and channel_secret and channel_project_root else None)
    server = Server(channel_credentials=channel_credentials)
    while True:
        line = sys.stdin.buffer.readline(MAX_REQUEST_BYTES+1)
        if not line:
            break
        request_id = None
        try:
            if len(line)>MAX_REQUEST_BYTES:
                while line and not line.endswith(b'\n'):
                    line = sys.stdin.buffer.readline(MAX_REQUEST_BYTES+1)
                raise ValueError
            item = json.loads(line, object_pairs_hook=reject_duplicates)
            if not isinstance(item, dict) or item.get('jsonrpc')!='2.0' or not isinstance(item.get('method'),str):
                raise ProtocolError(-32600,'Invalid JSON-RPC request. Use an object with jsonrpc=2.0, a string method, and a string or integer id for requests.')
            request_id = item.get('id')
            if 'id' in item and (type(request_id) not in (str,int) or isinstance(request_id,str) and len(request_id)>128):
                request_id = None
                raise ProtocolError(-32600,'Request id must be a string of at most 128 characters or an integer; null and boolean ids are not valid MCP request ids.')
            if 'id' not in item:
                continue
            result = server.dispatch(item['method'],item.get('params',{}),request_id=request_id)
            reply = dict(jsonrpc='2.0',id=request_id,result=result)
        except ProtocolError as exc:
            reply = dict(jsonrpc='2.0',id=request_id,error=dict(code=exc.code,message=exc.message))
        except (ValueError, UnicodeError, RecursionError):
            reply = dict(jsonrpc='2.0',id=None,error=dict(code=-32700,message='Cannot parse the request. Send one valid UTF-8 JSON object per line without duplicate keys; keep each line within 2,000,000 bytes. Nothing in this frame was executed.'))
        except Exception:
            _log_unexpected('dispatch')
            reply = dict(jsonrpc='2.0',id=request_id,error=dict(code=-32603,message='The server failed to complete this request. Do not change arguments to guess a repair. Check the local server; a write may be uncertain, so preserve and retry only its exact original request and delivery key.'))
        sys.stdout.write(json.dumps(reply,ensure_ascii=False)+'\n')
        sys.stdout.flush()
