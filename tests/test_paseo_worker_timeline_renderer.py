from pathlib import Path


SOURCE = Path(__file__).parents[1] / "integrations" / "paseo" / "index.client.tsx"
SELECTION = Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-selection.ts"


def test_paseo_worker_timeline_row_uses_supported_agent_context_and_panel_navigation():
    source = SOURCE.read_text(encoding="utf-8")
    assert 'query: { itemType: "tool_call" }' in source
    assert 'item.callId.startsWith("worker:")' in source
    assert 'status: z.enum(["running", "completed", "canceled", "failed"])' in source
    # The result section and its status both render with zero RPC once the
    # host has replayed the runtime's embedded projection (`item.data.result`,
    # `resolveWorkerCardStatus`'s title channel) -- `readWorkerResults` is
    # only the batched fallback for an older item. See client/use-worker-result.ts.
    # A settled durable attempt status (polled only while the host still
    # shows the card running) settles a card the host left stale after a
    # plugin reload.
    assert 'resolveDurableWorkerCardStatus(item.data.title, item.data.status, usage?.status)' in source
    assert 'workerCardMetadata(item.data.title, resolvedStatus)' in source
    assert 'const embedded = item.data.result ?? null;' in source
    assert 'useFallbackWorkerResult(workspaceId, agentId, item.data.assignmentRef, !isWorking && !embedded)' in source
    display = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-display.ts").read_text(encoding="utf-8")
    assert 'return "cancelled"' in display and 'return "working"' in display
    assert '>Worker</Text>' in source and '{metadata.workerName}' in source
    assert '{metadata.subtitle}' in source
    assert 'Open Workers' not in source
    assert 'routeParts.slice(1)' in display
    assert 'useAgent(props.agentId, (agent) => agent.workspaceId)' in source
    assert 'client.openPanel("workers", { workspaceId, agentId: props.agentId, location: "workspace" })' in source
    assert 'locations: ["workspace"]' in source
    assert 'const refreshCatalogue = coalesced(async () => {' in source
    assert 'const candidates = new Map<string, Candidate>()' in source
    assert 'for (const agentId of candidates.keys())' in source
    assert 'the popover owns the empty state' in source
    assert 'refreshVisibility' not in source
    assert 'if (!candidates.has(agentId))' in source
    assert 'selectWorker(agentId, item.data.assignmentRef)' in source


def test_paseo_pill_catalogue_lists_active_agents_once_by_workspace():
    """One listing of every active agent matched to its own workspace, not
    one project-filtered listing per workspace."""
    source = SOURCE.read_text(encoding="utf-8")
    assert "projectKeys" in source  # only in the explanatory comment
    assert "filter: { projectKeys" not in source
    assert 'client.paseo.agents.list({' in source
    assert 'scope: "active"' in source
    assert 'workspaces.get(entry.agent.workspaceId)' in source
    assert 'result.pageInfo.nextCursor' in source
    # A pill that fails to register must not be swallowed as "empty catalogue".
    assert source.index("updateRegistrations();\n  });") > source.index("      return;\n    }")


def test_paseo_pill_catalogue_is_re_read_whatever_the_load_order():
    """Loaded before the host connection was up, the first catalogue read
    failed and `subscribe()` relayed no daemon update in the next 15s (it
    only forwards updates something else asked the daemon to stream), so
    the pills never appeared. A steady re-read, cleared on cleanup, keeps
    the catalogue current regardless."""
    source = SOURCE.read_text(encoding="utf-8")
    assert "const CATALOGUE_REFRESH_MS = 5000;" in source
    assert "setInterval(() => { void refreshCatalogue(); }, CATALOGUE_REFRESH_MS)" in source
    assert "clearInterval(poll);" in source


def test_paseo_pill_catalogue_refresh_never_drops_a_subscription_event():
    """A notification arriving mid-refresh used to be discarded outright,
    so a just-created agent's Workers/Pipeline pills never appeared."""
    source = SOURCE.read_text(encoding="utf-8")
    assert 'import { coalesced } from "./client/coalesce.ts";' in source
    assert 'const refreshCatalogue = coalesced(async () => {' in source
    assert 'catalogueInFlight' not in source
    helper = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "coalesce.ts").read_text(encoding="utf-8")
    assert 'export function coalesced' in helper
    assert 'queued = true;' in helper


def test_paseo_worker_selection_is_plugin_owned_shared_state():
    source = SELECTION.read_text(encoding="utf-8")
    assert 'write(handoffKey(agentId), assignmentRef)' in source
    assert 'export function selectedWorker(agentId: string): string | null' in source


def test_paseo_workers_button_and_popover_are_removed_while_pipeline_remains():
    """The Workers composer pill/popover duplicated the Pipeline panel and was
    removed by user steer; the Pipeline panel/pill and worker detail panel
    (opened from timeline cards and Pipeline entries) must remain intact."""
    source = SOURCE.read_text(encoding="utf-8")
    assert not (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover.tsx").exists()
    assert not (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-history-poll.ts").exists()
    assert "WorkerPopover" not in source
    assert 'id: `workers-${agentId}`' not in source
    # The worker detail panel/tab and its command-center entry stay: worker
    # cards in the chat timeline and Pipeline's onOpenWorker still open it.
    assert 'id: "workers"' in source
    assert 'title: "Workers"' in source
    assert 'Component: WorkerPanel' in source
    # Pipeline stays fully intact: panel, composer pill and command item.
    assert 'id: "pipeline"' in source
    assert 'id: `pipeline-${agentId}`' in source
    assert 'title: "Pipeline"' in source
    status = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover-status.ts").read_text(encoding="utf-8")
    assert "orderPopoverWorkers" not in status
    assert "export function workerPopoverStatus" in status


def test_worker_history_terminal_status_is_independent_of_bounded_event_page():
    server = (Path(__file__).parents[1] / "integrations" / "paseo" / "server" / "worker-history.ts").read_text(encoding="utf-8")
    assert "terminal marker independently" in server
    assert "event_type in ('worker_result','assignment_failed','assignment_cancelled') order by id" in server
    assert "terminal_priority = {'worker_result': 2, 'assignment_failed': 1, 'assignment_cancelled': 1}" in server
    assert "terminal_type == 'assignment_cancelled'" in server


def test_paseo_pipeline_pill_remains_agent_scoped_across_shared_workspaces():
    source = SOURCE.read_text(encoding="utf-8")
    assert 'client.addComposerPill({' in source
    assert 'client.addHeaderButton({' not in source
    assert 'Composer pills are agent-scoped' in source
    assert 'workspace-scoped' in source
    # The registration key and contribution both retain the agent identity;
    # a second agent in the same workspace therefore gets an isolated pill.
    assert 'id: `pipeline-${agentId}`' in source
    assert 'agentId,' in source
    assert 'kind: "popover"' in source


def test_worker_card_assignment_key_is_resolved_before_attempt_selection():
    panel = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-panel.tsx").read_text(encoding="utf-8")
    server = (Path(__file__).parents[1] / "integrations" / "paseo" / "server" / "worker-history.ts").read_text(encoding="utf-8")
    # These are deliberately different namespaces: the card key is the
    # delegation idempotency key, while the graph and engine attempt IDs are
    # what the history response exposes for opening an attempt.
    assert 'assignmentRef: selectedAssignmentRef ?? undefined' in panel
    assert 'const attempt = assignment?.attempts[assignment.attempts.length - 1]' in panel
    assert 'a.assignment_id=? or a.idempotency_key=?' in server
    assert "json_extract(e.payload_json, '$.plan_assignment_ref')" in server
    assert 'work_assignment_attempts where assignment_id=? and attempt_id=?' in server


def test_worker_cards_use_resolved_role_instead_of_internal_delegate_ref():
    history = (Path(__file__).parents[1] / "integrations" / "paseo" / "shared" / "worker-history.ts").read_text(encoding="utf-8")
    display = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-display.ts").read_text(encoding="utf-8")
    panel = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-panel.tsx").read_text(encoding="utf-8")
    server = (Path(__file__).parents[1] / "integrations" / "paseo" / "server" / "worker-history.ts").read_text(encoding="utf-8")
    assert "role: z.string().min(1).optional()" in history
    assert "export function workerDisplayName" in display
    assert "delegate:[0-9a-f-]+" in display
    assert "candidate_role" in server
    assert "assignment_result['role'] = role" in server
    assert "workerDisplayName(assignment)" in panel
    assert "workerAssignmentLine(assignment" in panel


def test_worker_panel_seeds_selection_before_first_history_request():
    panel = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-panel.tsx").read_text(encoding="utf-8")
    assert 'useState<string | null>(() => saved ? saved.selected : selectedWorker(agentId))' in panel
    assert 'useState<string | null>(() => saved ? null : selectedWorker(agentId))' in panel
    assert 'setSelected(assignmentRef)' in panel
    assert 'assignmentRef: selectedAssignmentRef ?? undefined' in panel
