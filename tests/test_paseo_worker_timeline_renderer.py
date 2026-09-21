from pathlib import Path


SOURCE = Path(__file__).parents[1] / "integrations" / "paseo" / "index.client.tsx"
SELECTION = Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-selection.ts"


def test_paseo_worker_timeline_row_uses_supported_agent_context_and_panel_navigation():
    source = SOURCE.read_text(encoding="utf-8")
    assert 'query: { itemType: "tool_call" }' in source
    assert 'item.callId.startsWith("worker:")' in source
    assert 'status: z.enum(["running", "completed", "canceled", "failed"])' in source
    assert 'workerCardMetadata(item.data.title, item.data.status)' in source
    display = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-display.ts").read_text(encoding="utf-8")
    assert 'return "cancelled"' in display and 'return "working"' in display
    assert '>Worker</Text>' in source and '{metadata.workerName}' in source
    assert '{metadata.subtitle}' in source
    assert 'Open Workers' not in source
    assert 'routeParts.slice(1)' in display
    assert 'useAgent(props.agentId, (agent) => agent.workspaceId)' in source
    assert 'client.openPanel("workers", { workspaceId, agentId: props.agentId, location: "workspace" })' in source
    assert 'locations: ["workspace"]' in source
    assert 'let catalogueInFlight = false' in source
    assert 'const candidates = new Map<string, Candidate>()' in source
    assert 'for (const agentId of candidates.keys())' in source
    assert 'the popover owns the empty state' in source
    assert 'refreshVisibility' not in source
    assert 'if (!candidates.has(agentId))' in source
    assert 'selectWorker(agentId, item.data.assignmentRef)' in source


def test_paseo_worker_selection_is_plugin_owned_shared_state():
    source = SELECTION.read_text(encoding="utf-8")
    assert 'write(handoffKey(agentId), assignmentRef)' in source
    assert 'export function selectedWorker(agentId: string): string | null' in source


def test_paseo_workers_pill_uses_supported_popover_and_opens_selected_assignment():
    source = SOURCE.read_text(encoding="utf-8")
    popover = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover.tsx").read_text(encoding="utf-8")
    assert 'kind: "popover"' in source
    assert 'Content: (props) => <WorkerPopover' in source
    assert 'client.openPanel("workers", { workspaceId: candidate.workspace.id, agentId, location: "workspace" })' in source
    assert 'const ref = assignment.assignmentRef ?? assignment.id' in popover
    assert 'close(); onOpenWorker(ref)' in popover
    assert 'workerPopoverStatus(assignment)' in popover
    assert 'workerAssignmentLine(assignment, workerPopoverStatus(assignment))' in popover
    assert 'history?.available && assignments.length === 0' in popover
    assert 'No workers in this session.' in popover


def test_paseo_workers_popover_refreshes_terminal_status_and_is_mobile_bounded():
    popover = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover.tsx").read_text(encoding="utf-8")
    status = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover-status.ts").read_text(encoding="utf-8")
    assert 'setInterval(() => { void refresh(); }, 750)' in popover
    assert 'height: layout.compact ? compactHeight : undefined' not in popover
    assert 'ScrollView' not in popover
    assert 'eventTypes.has("assignment_cancelled")' in status


def test_worker_history_terminal_status_is_independent_of_bounded_event_page():
    server = (Path(__file__).parents[1] / "integrations" / "paseo" / "server" / "worker-history.ts").read_text(encoding="utf-8")
    assert "terminal marker independently" in server
    assert "event_type in ('worker_result','assignment_failed','assignment_cancelled') order by id" in server
    assert "terminal_priority = {'worker_result': 2, 'assignment_failed': 1, 'assignment_cancelled': 1}" in server
    assert "terminal_type == 'assignment_cancelled'" in server


def test_paseo_worker_popover_is_a_flat_compact_worker_list():
    popover = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover.tsx").read_text(encoding="utf-8")
    assert 'borderWidth: 1' not in popover
    assert 'compactHeight' not in popover
    assert 'workerAssignmentLine(assignment, workerPopoverStatus(assignment))' in popover
    assert 'borderBottomWidth: index === assignments.length - 1 ? 0 : 1' in popover
    assert 'Loading workers…' in popover
    assert '>Workers</Text>' not in popover


def test_paseo_worker_popover_has_no_unrequested_mutating_action():
    popover = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover.tsx").read_text(encoding="utf-8")
    assert 'Hide finished workers' not in popover
    assert 'Show finished workers' not in popover


def test_paseo_worker_popover_relies_on_host_frame_and_native_row_typography():
    popover = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover.tsx").read_text(encoding="utf-8")
    assert 'Icon name=' not in popover
    assert 'fontSize: 16, fontWeight: "400"' in popover
    assert 'fontSize: 13' in popover
    assert 'borderWidth: 1' not in popover
    assert 'borderRadius: 10' not in popover
    assert 'width: layout.compact ? undefined : 340' in popover
    assert 'alignSelf: layout.compact ? "stretch" : "auto"' in popover
    assert 'marginHorizontal: -12' in popover
    assert 'flexDirection: "column", alignItems: "stretch", gap: 2' in popover


def test_paseo_workers_control_remains_agent_scoped_across_shared_workspaces():
    source = SOURCE.read_text(encoding="utf-8")
    assert 'client.addComposerPill({' in source
    assert 'client.addHeaderButton({' not in source
    assert 'Composer pills are agent-scoped' in source
    assert 'workspace-scoped' in source
    # The registration key and contribution both retain the agent identity;
    # a second agent in the same workspace therefore gets an isolated pill.
    assert 'id: `workers-${agentId}`' in source
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
    popover = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-popover.tsx").read_text(encoding="utf-8")
    server = (Path(__file__).parents[1] / "integrations" / "paseo" / "server" / "worker-history.ts").read_text(encoding="utf-8")
    assert "role: z.string().min(1).optional()" in history
    assert "export function workerDisplayName" in display
    assert "delegate:[0-9a-f-]+" in display
    assert "candidate_role" in server
    assert "assignment_result['role'] = role" in server
    assert "workerDisplayName(assignment)" in panel
    assert "workerAssignmentLine(assignment" in popover


def test_worker_panel_seeds_selection_before_first_history_request():
    panel = (Path(__file__).parents[1] / "integrations" / "paseo" / "client" / "worker-panel.tsx").read_text(encoding="utf-8")
    assert 'useState<string | null>(() => saved ? saved.selected : selectedWorker(agentId))' in panel
    assert 'useState<string | null>(() => saved ? null : selectedWorker(agentId))' in panel
    assert 'setSelected(assignmentRef)' in panel
    assert 'assignmentRef: selectedAssignmentRef ?? undefined' in panel
