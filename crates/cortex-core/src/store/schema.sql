-- Generated offline from the current Python Store; do not hand edit.
CREATE TABLE IF NOT EXISTS tasks (
 id TEXT PRIMARY KEY, project_root TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tasks_by_project ON tasks(project_root);
CREATE TABLE IF NOT EXISTS thread_bindings (
 thread_id TEXT PRIMARY KEY, parent_thread_id TEXT,
 task_id TEXT NOT NULL REFERENCES tasks(id)
);
CREATE TABLE IF NOT EXISTS reports (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 kind TEXT NOT NULL, filename TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_cursors (
 thread_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), cursor TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_messages (
 thread_id TEXT NOT NULL, message_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES tasks(id),
 report_id TEXT NOT NULL REFERENCES reports(id), digest TEXT NOT NULL,
 PRIMARY KEY(thread_id,message_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS one_pipeline ON reports(task_id) WHERE kind='pipeline';
CREATE TABLE IF NOT EXISTS editions (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 report_id TEXT NOT NULL REFERENCES reports(id),
 title TEXT NOT NULL, summary TEXT NOT NULL, author TEXT NOT NULL,
 updated_at TEXT NOT NULL, digest TEXT NOT NULL, size_bytes INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS report_editions ON editions(report_id,sequence);
CREATE TABLE IF NOT EXISTS governance (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), mode TEXT NOT NULL,
 report_id TEXT NOT NULL REFERENCES reports(id)
);
CREATE TABLE IF NOT EXISTS deliveries (
 scope TEXT NOT NULL, operation TEXT NOT NULL, delivery_key TEXT NOT NULL,
 digest TEXT NOT NULL, content_digest TEXT NOT NULL, response TEXT NOT NULL,
 PRIMARY KEY(scope,operation,delivery_key)
);
CREATE TABLE IF NOT EXISTS drafts (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 owner_thread_id TEXT NOT NULL, kind TEXT NOT NULL, template TEXT NOT NULL,
 path TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL,
 device INTEGER NOT NULL, inode INTEGER NOT NULL,
 published_report_id TEXT REFERENCES reports(id), published_digest TEXT
);
CREATE TABLE IF NOT EXISTS pending_source_deletions (
 task_id TEXT NOT NULL REFERENCES tasks(id), draft_id TEXT NOT NULL REFERENCES drafts(id),
 path TEXT NOT NULL, digest TEXT NOT NULL, PRIMARY KEY(task_id,draft_id)
);
CREATE TABLE IF NOT EXISTS pending_deletions (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id)
);
CREATE TRIGGER IF NOT EXISTS editions_no_update BEFORE UPDATE ON editions BEGIN
 SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS editions_no_delete BEFORE DELETE ON editions
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d JOIN reports r ON r.task_id=d.task_id
 WHERE r.id=OLD.report_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS binding_receipts (
 thread_id TEXT PRIMARY KEY REFERENCES thread_bindings(thread_id), receipt TEXT NOT NULL UNIQUE,
 state TEXT NOT NULL DEFAULT 'cortex', updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_resets (
 thread_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), pause_turn TEXT
);
CREATE TABLE IF NOT EXISTS source_turns (
 task_id TEXT NOT NULL REFERENCES tasks(id), thread_id TEXT NOT NULL,
 turn_id TEXT NOT NULL, report_id TEXT NOT NULL REFERENCES reports(id), digest TEXT NOT NULL,
 PRIMARY KEY(thread_id,turn_id,report_id)
);
CREATE TABLE IF NOT EXISTS source_revisions (
 task_id TEXT NOT NULL REFERENCES tasks(id), revision INTEGER NOT NULL,
 thread_id TEXT NOT NULL, message_id TEXT NOT NULL, report_id TEXT NOT NULL REFERENCES reports(id),
 attachments TEXT NOT NULL DEFAULT '[]', PRIMARY KEY(task_id,revision), UNIQUE(thread_id,message_id)
);
CREATE TABLE IF NOT EXISTS report_provenance (
 edition_sequence INTEGER PRIMARY KEY REFERENCES editions(sequence), source_revision INTEGER NOT NULL,
 artifacts TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS draft_revisions (
 draft_id TEXT PRIMARY KEY REFERENCES drafts(id), source_revision INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS task_changes (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL REFERENCES tasks(id),
 kind TEXT NOT NULL, reference TEXT, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS changes_by_task ON task_changes(task_id,sequence);
CREATE TABLE IF NOT EXISTS attempt_revisions (
 attempt_id TEXT NOT NULL, captured_at TEXT NOT NULL, phase TEXT NOT NULL,
 kind TEXT NOT NULL, head TEXT, dirty_digest TEXT, detail TEXT, manifest_json TEXT
);
CREATE INDEX IF NOT EXISTS attempt_revisions_by_attempt ON attempt_revisions(attempt_id,captured_at);
CREATE TABLE IF NOT EXISTS format_history (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, action TEXT NOT NULL,
 from_version INTEGER NOT NULL, to_version INTEGER NOT NULL,
 backup_path TEXT NOT NULL, backup_digest TEXT NOT NULL, at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS trusted_channels (
 channel_id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 principal_thread_id TEXT NOT NULL, parent_principal_thread_id TEXT,
 role TEXT NOT NULL, assignment_id TEXT, attempt_id TEXT,
 workspace_binding_id TEXT, context_generation_id TEXT,
 owner_epoch INTEGER NOT NULL, issued_at TEXT NOT NULL,
 expires_at TEXT, revoked_at TEXT
);
CREATE TABLE IF NOT EXISTS source_events (
 origin TEXT NOT NULL, event_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES tasks(id),
 captured_at TEXT NOT NULL, meta_json TEXT, ledger_state TEXT, PRIMARY KEY(origin,event_id)
);
CREATE TABLE IF NOT EXISTS task_owners (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id), owner_ref TEXT NOT NULL,
 epoch INTEGER NOT NULL, acquired_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS task_route_selections (
 task_id TEXT NOT NULL REFERENCES tasks(id), sequence INTEGER NOT NULL,
 route_id TEXT NOT NULL, label_at_selection TEXT NOT NULL,
 supported_parameters TEXT NOT NULL, catalog_snapshot_id TEXT NOT NULL,
 evidence TEXT NOT NULL, selected_at TEXT NOT NULL,
 PRIMARY KEY(task_id,sequence)
);
CREATE INDEX IF NOT EXISTS task_route_selections_by_task ON task_route_selections(task_id,sequence);
CREATE TRIGGER IF NOT EXISTS task_route_selections_no_update BEFORE UPDATE ON task_route_selections
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS task_route_selections_no_delete BEFORE DELETE ON task_route_selections
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS project_configuration_defaults (
 project_root TEXT PRIMARY KEY, defaults TEXT NOT NULL, ceiling TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS configuration_revisions (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), revision INTEGER NOT NULL,
 route_id TEXT NOT NULL, route_engine TEXT NOT NULL, route_connection_id TEXT NOT NULL,
 parameters TEXT NOT NULL, ceiling_snapshot TEXT NOT NULL,
 state TEXT NOT NULL, confirmation_evidence_ref TEXT, error_code TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(task_id,revision)
);
CREATE INDEX IF NOT EXISTS configuration_revisions_by_task ON configuration_revisions(task_id,revision);
CREATE TRIGGER IF NOT EXISTS configuration_revisions_guard BEFORE UPDATE ON configuration_revisions
BEGIN
 SELECT CASE
  WHEN NEW.id!=OLD.id OR NEW.task_id!=OLD.task_id OR NEW.revision!=OLD.revision
   OR NEW.route_id!=OLD.route_id OR NEW.route_engine!=OLD.route_engine
   OR NEW.route_connection_id!=OLD.route_connection_id OR NEW.parameters!=OLD.parameters
   OR NEW.ceiling_snapshot!=OLD.ceiling_snapshot OR NEW.created_at!=OLD.created_at
  THEN RAISE(ABORT,'immutable')
  WHEN OLD.state='selected' AND NEW.state NOT IN ('selected','pending_next_use','rejected')
  THEN RAISE(ABORT,'invalid_transition')
  WHEN OLD.state='pending_next_use' AND NEW.state NOT IN ('pending_next_use','active','failed','indeterminate')
  THEN RAISE(ABORT,'invalid_transition')
  WHEN OLD.state='indeterminate' AND NEW.state NOT IN ('indeterminate','active','failed')
  THEN RAISE(ABORT,'invalid_transition')
  WHEN OLD.state IN ('active','rejected','failed') AND NEW.state!=OLD.state
  THEN RAISE(ABORT,'invalid_transition')
  WHEN NEW.state='active' AND (NEW.confirmation_evidence_ref IS NULL OR NEW.confirmation_evidence_ref='')
  THEN RAISE(ABORT,'missing_confirmation_evidence')
  WHEN NEW.state='active' AND EXISTS (
   SELECT 1 FROM configuration_revisions o WHERE o.task_id=NEW.task_id AND o.revision>NEW.revision
    AND o.state NOT IN ('rejected','failed')
  )
  THEN RAISE(ABORT,'stale_reconcile_target')
 END;
END;
CREATE TRIGGER IF NOT EXISTS configuration_revisions_no_delete BEFORE DELETE ON configuration_revisions
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS acp_host_sessions (
 session_id TEXT PRIMARY KEY, project_root TEXT NOT NULL,
 task_id TEXT REFERENCES tasks(id), client_info TEXT NOT NULL,
 client_capabilities TEXT NOT NULL, mcp_servers TEXT NOT NULL,
 created_at TEXT NOT NULL, closed_at TEXT
);
CREATE INDEX IF NOT EXISTS acp_host_sessions_by_task ON acp_host_sessions(task_id);
CREATE TRIGGER IF NOT EXISTS acp_host_sessions_task_immutable
BEFORE UPDATE OF task_id ON acp_host_sessions
WHEN OLD.task_id IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS acp_session_access_defaults (
 session_id TEXT PRIMARY KEY REFERENCES acp_host_sessions(session_id) ON DELETE CASCADE,
 mode TEXT NOT NULL CHECK(mode IN ('development','guarded','auto')),
 selected_mode TEXT CHECK(selected_mode IN ('development','guarded','auto')),
 selection_origin TEXT
);
CREATE TABLE IF NOT EXISTS task_workspaces (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 workspace_root TEXT NOT NULL,
 recorded_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS acp_session_workspaces (
 session_id TEXT PRIMARY KEY REFERENCES acp_host_sessions(session_id),
 workspace_root TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS delegation_provisioning (
 idempotency_key TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_id TEXT, attempt_id TEXT, role_id TEXT NOT NULL,
 step TEXT NOT NULL, status TEXT NOT NULL,
 workspace_binding_id TEXT, channel_id TEXT, config_revision TEXT,
 context_snapshot_ref TEXT, engine_session_ref TEXT, instruction_delivery TEXT,
 error_code TEXT, owner_epoch INTEGER NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS delegation_provisioning_by_task ON delegation_provisioning(task_id);
CREATE TABLE IF NOT EXISTS delegation_effects (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 idempotency_key TEXT NOT NULL REFERENCES delegation_provisioning(idempotency_key),
 step TEXT NOT NULL, phase TEXT NOT NULL CHECK(phase IN ('intent','outcome')),
 detail_json TEXT NOT NULL, at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS delegation_effects_by_key ON delegation_effects(idempotency_key,step,id);
CREATE TABLE IF NOT EXISTS work_requirements (
 requirement_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 current_revision INTEGER NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS work_requirements_by_task ON work_requirements(task_id);
CREATE TABLE IF NOT EXISTS work_requirement_revisions (
 requirement_id TEXT NOT NULL REFERENCES work_requirements(requirement_id),
 revision INTEGER NOT NULL,
 statement TEXT NOT NULL,
 criteria_json TEXT NOT NULL,
 digest TEXT NOT NULL,
 requirement_type TEXT NOT NULL DEFAULT 'general',
 created_at TEXT NOT NULL,
 PRIMARY KEY(requirement_id, revision)
);
CREATE TABLE IF NOT EXISTS work_assignments (
 assignment_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 node_key TEXT NOT NULL,
 requirement_bindings_json TEXT NOT NULL,
 candidate_ref TEXT,
 status TEXT NOT NULL,
 requirement_state TEXT NOT NULL DEFAULT 'structured',
 idempotency_key TEXT NOT NULL,
 created_at TEXT NOT NULL,
 UNIQUE(task_id, idempotency_key)
);
CREATE INDEX IF NOT EXISTS work_assignments_by_task ON work_assignments(task_id);
CREATE TABLE IF NOT EXISTS work_assignment_predecessors (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_id TEXT NOT NULL REFERENCES work_assignments(assignment_id),
 predecessor_assignment_id TEXT NOT NULL REFERENCES work_assignments(assignment_id),
 predecessor_candidate_ref TEXT NOT NULL,
 PRIMARY KEY(assignment_id, predecessor_assignment_id)
);
CREATE INDEX IF NOT EXISTS work_predecessors_by_predecessor
 ON work_assignment_predecessors(predecessor_assignment_id);
CREATE TABLE IF NOT EXISTS work_assignment_attempts (
 attempt_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_id TEXT NOT NULL REFERENCES work_assignments(assignment_id),
 attempt_no INTEGER NOT NULL,
 predecessor_attempt_id TEXT REFERENCES work_assignment_attempts(attempt_id),
 input_candidate_ref TEXT,
 requirement_bindings_json TEXT NOT NULL,
 status TEXT NOT NULL,
 failure_reason TEXT,
 idempotency_key TEXT NOT NULL,
 created_at TEXT NOT NULL,
 UNIQUE(task_id, idempotency_key),
 UNIQUE(assignment_id, attempt_no)
);
CREATE INDEX IF NOT EXISTS work_attempts_by_assignment
 ON work_assignment_attempts(assignment_id, attempt_no);
CREATE TABLE IF NOT EXISTS work_graph_operations (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 idempotency_key TEXT NOT NULL,
 operation TEXT NOT NULL,
 request_json TEXT NOT NULL DEFAULT '{}',
 response_json TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(task_id, idempotency_key)
);
CREATE TABLE IF NOT EXISTS task_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 event_type TEXT NOT NULL CHECK(event_type IN (
   'delegation_requested','assignment_ready','assignment_failed',
   'assignment_cancel_requested','assignment_cancelled',
   'worker_result','worker_question','coordinator_clarification',
   'coordinator_decision','user_message','progress'
 )),
 assignment_ref TEXT, dedup_key TEXT NOT NULL,
 payload_json TEXT NOT NULL, origin TEXT NOT NULL,
 created_at TEXT NOT NULL, consumed_by TEXT,
 superseded INTEGER NOT NULL DEFAULT 0,
 delivered_at TEXT,
 UNIQUE(task_id, dedup_key)
);
CREATE INDEX IF NOT EXISTS task_events_by_task ON task_events(task_id, id);
CREATE INDEX IF NOT EXISTS task_events_by_activity_ref ON task_events(task_id, json_extract(payload_json,'$.activity_ref'), id);
CREATE INDEX IF NOT EXISTS task_events_by_assignment ON task_events(task_id, assignment_ref);
CREATE INDEX IF NOT EXISTS task_events_by_type ON task_events(task_id, event_type, id);
CREATE INDEX IF NOT EXISTS task_events_unconsumed ON task_events(task_id, id) WHERE consumed_by IS NULL AND superseded=0;
CREATE TABLE IF NOT EXISTS task_orchestration (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 state TEXT NOT NULL,
 coordinator_attempt_ref TEXT,
 updated_at TEXT NOT NULL,
 wait_reason TEXT,
 wait_detail_json TEXT,
 no_progress_count INTEGER NOT NULL DEFAULT 0,
 requirement_revision INTEGER NOT NULL DEFAULT 0,
 stopped_at TEXT,
 language TEXT
);
CREATE TABLE IF NOT EXISTS runtime_tasks (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 state TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0,
 goal TEXT NOT NULL, plan_json TEXT, synthesis TEXT,
 review_cycle INTEGER NOT NULL DEFAULT 0, blocker TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_nodes (
 task_id TEXT NOT NULL REFERENCES runtime_tasks(task_id),
 revision INTEGER NOT NULL, node_id TEXT NOT NULL,
 role TEXT NOT NULL, objective TEXT NOT NULL, dependencies_json TEXT NOT NULL,
 scope_json TEXT NOT NULL, stage TEXT NOT NULL, status TEXT NOT NULL,
 assignment_ref TEXT UNIQUE, result_json TEXT,
 PRIMARY KEY(task_id,revision,node_id)
);
CREATE TABLE IF NOT EXISTS runtime_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL REFERENCES runtime_tasks(task_id),
 type TEXT NOT NULL, actor TEXT NOT NULL, payload_json TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS runtime_events_task ON runtime_events(task_id,id);
CREATE INDEX IF NOT EXISTS runtime_events_type ON runtime_events(type,task_id,id);
CREATE TABLE IF NOT EXISTS runtime_requests (
 task_id TEXT NOT NULL, request_key TEXT NOT NULL, actor TEXT NOT NULL,
 operation TEXT NOT NULL, digest TEXT NOT NULL, result_json TEXT NOT NULL,
 PRIMARY KEY(task_id,actor,request_key)
);
CREATE INDEX IF NOT EXISTS runtime_requests_by_operation ON runtime_requests(task_id,actor,operation);
CREATE TABLE IF NOT EXISTS capacity_reservations (
 reservation_id TEXT PRIMARY KEY,
 connection_id TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 attempt_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('coordinator','worker')),
 state TEXT NOT NULL CHECK(state IN ('reserved','ambiguous','released')),
 reserved_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 release_observation TEXT
);
CREATE INDEX IF NOT EXISTS capacity_by_connection ON capacity_reservations(connection_id, state);
CREATE INDEX IF NOT EXISTS capacity_by_task ON capacity_reservations(task_id);
CREATE INDEX IF NOT EXISTS capacity_by_attempt ON capacity_reservations(attempt_id);
CREATE TABLE IF NOT EXISTS tool_result_observations (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT NOT NULL,
 tool_call_ref TEXT NOT NULL,
 kind TEXT,
 title TEXT,
 status TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tool_result_observations_by_attempt ON tool_result_observations(attempt_id, tool_call_ref, id);
CREATE INDEX IF NOT EXISTS tool_result_observations_by_task ON tool_result_observations(task_id);
CREATE TABLE IF NOT EXISTS report_read_observations (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT NOT NULL,
 report_id TEXT NOT NULL REFERENCES reports(id),
 request_id TEXT NOT NULL,
 outcome TEXT NOT NULL CHECK (outcome IN ('success')),
 created_at TEXT NOT NULL,
 PRIMARY KEY (task_id, attempt_id, report_id, request_id)
);
CREATE INDEX IF NOT EXISTS report_read_observations_by_attempt ON report_read_observations(task_id, attempt_id, report_id);
CREATE TABLE IF NOT EXISTS permission_receipts (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT NOT NULL,
 tool_call_ref TEXT NOT NULL,
 command_sha256 TEXT NOT NULL,
 command_text TEXT NOT NULL,
 host_outcome TEXT NOT NULL CHECK (host_outcome IN ('allow_once','reject_once','cancelled')),
 created_at TEXT NOT NULL,
 PRIMARY KEY (task_id, attempt_id, tool_call_ref)
);
CREATE INDEX IF NOT EXISTS permission_receipts_by_attempt ON permission_receipts(task_id, attempt_id);
CREATE TABLE IF NOT EXISTS source_blocks (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 report_id TEXT NOT NULL REFERENCES reports(id),
 block_index INTEGER NOT NULL,
 kind TEXT NOT NULL,
 mime TEXT,
 name TEXT,
 uri TEXT,
 sha256 TEXT,
 size_bytes INTEGER,
 availability TEXT NOT NULL,
 reason TEXT,
 trust_label TEXT NOT NULL,
 body_ref TEXT,
 fetched_at TEXT,
 created_at TEXT NOT NULL,
 PRIMARY KEY(report_id, block_index)
);
CREATE INDEX IF NOT EXISTS source_blocks_by_task ON source_blocks(task_id);
CREATE TABLE IF NOT EXISTS source_block_reads (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 report_id TEXT NOT NULL,
 block_index INTEGER NOT NULL,
 observed_at TEXT NOT NULL,
 availability TEXT NOT NULL,
 reason TEXT
);
CREATE INDEX IF NOT EXISTS source_block_reads_by_block ON source_block_reads(report_id, block_index, id);
CREATE TABLE IF NOT EXISTS host_transcript (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 session_id TEXT NOT NULL REFERENCES acp_host_sessions(session_id),
 seq INTEGER NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('user_message','agent_message')),
 text TEXT NOT NULL,
 attachments_json TEXT,
 message_id TEXT,
 created_at TEXT NOT NULL,
 UNIQUE(session_id, seq)
);
CREATE INDEX IF NOT EXISTS host_transcript_by_session ON host_transcript(session_id, seq);
CREATE TABLE IF NOT EXISTS host_updates (
 session_id TEXT NOT NULL REFERENCES acp_host_sessions(session_id),
 seq INTEGER NOT NULL,
 update_json TEXT NOT NULL,
 created_at TEXT NOT NULL,
 UNIQUE(session_id, seq)
);
CREATE INDEX IF NOT EXISTS host_updates_by_session ON host_updates(session_id, seq);
CREATE TABLE IF NOT EXISTS pending_questions (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 dedup_key TEXT NOT NULL,
 text TEXT NOT NULL,
 options_json TEXT,
 status TEXT NOT NULL DEFAULT 'open',
 answer_text TEXT,
 answer_source TEXT,
 created_at TEXT NOT NULL,
 answered_at TEXT,
 UNIQUE(task_id, dedup_key)
);
CREATE INDEX IF NOT EXISTS pending_questions_by_task ON pending_questions(task_id, status, id);
CREATE TABLE IF NOT EXISTS pending_question_late_answers (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 question_id TEXT NOT NULL,
 task_id TEXT NOT NULL,
 answer_text TEXT NOT NULL,
 answer_source TEXT NOT NULL,
 received_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS task_language (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 language TEXT
);
CREATE TABLE IF NOT EXISTS task_stopped (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 stopped_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS context_generations (
 id TEXT PRIMARY KEY,
 engine_session_ref TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 recipient TEXT NOT NULL CHECK(recipient IN ('coordinator','worker')),
 sequence INTEGER NOT NULL,
 cause TEXT NOT NULL CHECK(cause IN (
   'session_open','observed_compaction','observed_reset',
   'session_replacement','controlled_boundary_refresh'
 )),
 evidence_source TEXT NOT NULL,
 evidence_id TEXT,
 created_at TEXT NOT NULL,
 UNIQUE(engine_session_ref, sequence)
);
CREATE INDEX IF NOT EXISTS context_generations_by_session ON context_generations(engine_session_ref, sequence);
CREATE INDEX IF NOT EXISTS context_generations_by_task ON context_generations(task_id);
CREATE TABLE IF NOT EXISTS compaction_owners (
 engine_session_ref TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 owner TEXT NOT NULL CHECK(owner IN ('engine_adapter','cortex_controlled','none')),
 tier TEXT NOT NULL CHECK(tier IN ('automatic','controlled_boundary','unqualified')),
 engine TEXT NOT NULL,
 engine_version TEXT NOT NULL,
 limitation TEXT,
 set_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS compaction_owners_by_task ON compaction_owners(task_id);
CREATE TABLE IF NOT EXISTS compaction_events_seen (
 engine_session_ref TEXT NOT NULL,
 evidence_id TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 phase TEXT NOT NULL,
 observed_at TEXT NOT NULL,
 generation_id TEXT,
 PRIMARY KEY (engine_session_ref, evidence_id)
);
CREATE INDEX IF NOT EXISTS compaction_events_seen_by_task ON compaction_events_seen(task_id);
CREATE TABLE IF NOT EXISTS capsule_deliveries (
 id TEXT PRIMARY KEY,
 recipient TEXT NOT NULL CHECK(recipient IN ('coordinator','worker')),
 context_generation_id TEXT NOT NULL REFERENCES context_generations(id),
 capsule_revision TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 engine_session_ref TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN (
   'built','transmitted','delivery_confirmed','continuation_observed','blocked','superseded'
 )),
 evidence_source TEXT NOT NULL,
 uncertainty TEXT,
 reason TEXT,
 retry_count INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 UNIQUE(recipient, context_generation_id, capsule_revision)
);
CREATE INDEX IF NOT EXISTS capsule_deliveries_by_task ON capsule_deliveries(task_id);
CREATE INDEX IF NOT EXISTS capsule_deliveries_by_session ON capsule_deliveries(engine_session_ref, id);
CREATE TABLE IF NOT EXISTS large_outputs (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 owner_scope TEXT NOT NULL,
 source_kind TEXT NOT NULL CHECK(source_kind IN ('terminal','tool_result')),
 source_ref TEXT,
 filename TEXT NOT NULL,
 byte_count INTEGER NOT NULL,
 truncated INTEGER NOT NULL CHECK(truncated IN (0,1)),
 exit_code INTEGER,
 signal TEXT,
 failure TEXT,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS large_outputs_by_task ON large_outputs(task_id);
CREATE INDEX IF NOT EXISTS large_outputs_by_owner ON large_outputs(task_id, owner_scope);
CREATE TABLE IF NOT EXISTS compaction_recovery_events (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 engine_session_ref TEXT NOT NULL,
 generation_id TEXT REFERENCES context_generations(id),
 before_tokens INTEGER NOT NULL,
 before_basis TEXT NOT NULL,
 after_tokens INTEGER NOT NULL,
 after_basis TEXT NOT NULL,
 recovered_tokens INTEGER NOT NULL,
 recovered_ratio REAL NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('ok','blocked')),
 reason TEXT,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS compaction_recovery_events_by_session
 ON compaction_recovery_events(engine_session_ref, id);
CREATE INDEX IF NOT EXISTS compaction_recovery_events_by_task ON compaction_recovery_events(task_id);
CREATE TABLE IF NOT EXISTS attempt_process_registry (
 attempt_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 role TEXT NOT NULL, engine_session_ref TEXT, connection_id TEXT,
 pid INTEGER NOT NULL, pgid INTEGER NOT NULL,
 start_ticks TEXT NOT NULL, boot_id TEXT NOT NULL,
 launch_command_digest TEXT NOT NULL,
 recorded_at TEXT NOT NULL, terminated_at TEXT, termination_reason TEXT
);
CREATE TABLE IF NOT EXISTS attempt_effective_context (
 attempt_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 role TEXT NOT NULL,
 selected_config_revision_id TEXT, effective_config_revision_id TEXT,
 route_engine TEXT NOT NULL, route_connection_id TEXT,
 resource_policy_json TEXT NOT NULL, recorded_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_snapshots (
 workspace_binding_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT NOT NULL,
 project_root TEXT NOT NULL,
 root_path TEXT,
 base_path TEXT,
 mode TEXT NOT NULL,
 manifest_json TEXT NOT NULL,
 base_kind TEXT NOT NULL,
 base_head TEXT,
 base_dirty_digest TEXT,
 created_at TEXT NOT NULL,
 cleaned_up_at TEXT,
 cleanup_reason TEXT
);
CREATE INDEX IF NOT EXISTS workspace_snapshots_by_attempt ON workspace_snapshots(attempt_id, created_at);
CREATE TABLE IF NOT EXISTS workspace_snapshot_lineage (
 workspace_binding_id TEXT PRIMARY KEY REFERENCES workspace_snapshots(workspace_binding_id),
 candidate_id TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_candidate_seeds (
 assignment_ref TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 candidate_id TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_integrations (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT NOT NULL,
 request_key TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 workspace_binding_id TEXT,
 status TEXT NOT NULL,
 applied_paths_json TEXT NOT NULL,
 conflict_paths_json TEXT NOT NULL,
 integrated_head TEXT,
 integrated_dirty_digest TEXT,
 host_buffer_state TEXT NOT NULL DEFAULT 'unknown',
 created_at TEXT NOT NULL,
 UNIQUE(assignment_ref, request_key)
);
CREATE TABLE IF NOT EXISTS workspace_integration_journal (
 journal_key TEXT NOT NULL,
 relpath TEXT NOT NULL,
 before_json TEXT NOT NULL,
 before_bytes BLOB,
 desired_json TEXT NOT NULL,
 state TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(journal_key, relpath)
);
CREATE TABLE IF NOT EXISTS configuration_route_extras (
 revision_id TEXT PRIMARY KEY REFERENCES configuration_revisions(id),
 task_id TEXT NOT NULL REFERENCES tasks(id),
 route_model TEXT NOT NULL,
 route_profile_json TEXT NOT NULL,
 route_details_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS configuration_route_extras_by_task ON configuration_route_extras(task_id);
CREATE TABLE IF NOT EXISTS attempt_route_audit (
 id TEXT PRIMARY KEY,
 attempt_id TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 role TEXT NOT NULL,
 engine TEXT NOT NULL,
 connection_id TEXT NOT NULL,
 model TEXT NOT NULL,
 params_fingerprint TEXT NOT NULL,
 config_revision_id TEXT,
 decision TEXT NOT NULL CHECK(decision IN ('authorized','fallback')),
 decision_detail_json TEXT NOT NULL,
 predecessor_attempt_id TEXT,
 recorded_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS attempt_route_audit_by_task ON attempt_route_audit(task_id, recorded_at);
CREATE INDEX IF NOT EXISTS attempt_route_audit_by_attempt ON attempt_route_audit(attempt_id);
CREATE TRIGGER IF NOT EXISTS attempt_route_audit_no_update BEFORE UPDATE ON attempt_route_audit
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS attempt_route_audit_no_delete BEFORE DELETE ON attempt_route_audit
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS fallback_consent_records (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 target_connection_id TEXT NOT NULL,
 target_provider TEXT NOT NULL,
 data_scope TEXT NOT NULL,
 granted_by TEXT NOT NULL,
 granted_at TEXT NOT NULL,
 revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS fallback_consent_by_task ON fallback_consent_records(task_id, target_connection_id);
CREATE TABLE IF NOT EXISTS observability_events (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 category TEXT NOT NULL,
 outcome TEXT NOT NULL,
 basis TEXT NOT NULL CHECK(basis IN ('observed','engine_reported','derived','estimated','unknown')),
 assignment_ref TEXT, attempt_id TEXT, engine_session_ref TEXT,
 context_generation_id TEXT, config_revision_id TEXT, connection_id TEXT,
 route_engine TEXT, source_event_ref TEXT,
 detail_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS observability_events_by_task ON observability_events(task_id, created_at);
CREATE INDEX IF NOT EXISTS observability_events_by_attempt ON observability_events(attempt_id);
CREATE TRIGGER IF NOT EXISTS observability_events_no_update BEFORE UPDATE ON observability_events
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS observability_events_no_delete BEFORE DELETE ON observability_events
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS usage_records (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT, engine_session_ref TEXT NOT NULL,
 call_sequence INTEGER NOT NULL,
 basis TEXT NOT NULL CHECK(basis IN ('observed','engine_reported','derived','estimated','unknown')),
 tokens_json TEXT NOT NULL DEFAULT '{}',
 stop_reason TEXT,
 failed INTEGER NOT NULL DEFAULT 0 CHECK(failed IN (0,1)),
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS usage_records_by_task ON usage_records(task_id, created_at);
CREATE INDEX IF NOT EXISTS usage_records_by_attempt ON usage_records(attempt_id, call_sequence);
CREATE TRIGGER IF NOT EXISTS usage_records_no_update BEFORE UPDATE ON usage_records
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS usage_records_no_delete BEFORE DELETE ON usage_records
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TABLE IF NOT EXISTS workspace_projects (
 project_id TEXT PRIMARY KEY, project_root TEXT NOT NULL UNIQUE,
 activated_by TEXT NOT NULL, activated_at TEXT NOT NULL,
 version INTEGER NOT NULL DEFAULT 1 CHECK(version>0)
);
CREATE TABLE IF NOT EXISTS workspace_tasks (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL UNIQUE, project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 description TEXT NOT NULL, version INTEGER NOT NULL DEFAULT 1 CHECK(version>0),
 execution_id TEXT, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS workspace_tasks_project ON workspace_tasks(project_id,sequence);
CREATE TABLE IF NOT EXISTS workspace_task_options (
 task_id TEXT PRIMARY KEY REFERENCES workspace_tasks(task_id), options_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_executions (
 execution_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES workspace_tasks(task_id),
 runtime_task_id TEXT NOT NULL UNIQUE, acp_session_id TEXT NOT NULL UNIQUE,
 owner_epoch INTEGER NOT NULL CHECK(owner_epoch>0),
 version INTEGER NOT NULL DEFAULT 1 CHECK(version>0), bound_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_starts (
 execution_id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE REFERENCES workspace_tasks(task_id),
 project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 actor_id TEXT NOT NULL, request_id TEXT NOT NULL,
 source_origin TEXT NOT NULL, source_event_id TEXT NOT NULL UNIQUE,
 state TEXT NOT NULL CHECK(state IN ('prepare','claimed','dispatched','unknown')),
 version INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
 UNIQUE(project_id,actor_id,request_id)
);
CREATE TABLE IF NOT EXISTS workspace_requests (
 project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 actor_id TEXT NOT NULL, request_id TEXT NOT NULL, operation TEXT NOT NULL,
 digest TEXT NOT NULL, receipt_json TEXT NOT NULL,
 PRIMARY KEY(project_id,actor_id,request_id)
);
CREATE TABLE IF NOT EXISTS workspace_outbox_state (
 project_id TEXT PRIMARY KEY REFERENCES workspace_projects(project_id),
 high_watermark INTEGER NOT NULL DEFAULT 0 CHECK(high_watermark>=0),
 retained_floor INTEGER NOT NULL DEFAULT 0 CHECK(retained_floor>=0 AND retained_floor<=high_watermark)
);
CREATE TABLE IF NOT EXISTS workspace_outbox (
 project_id TEXT NOT NULL REFERENCES workspace_projects(project_id),
 sequence INTEGER NOT NULL CHECK(sequence>0),
 event_id TEXT NOT NULL UNIQUE, entity_id TEXT NOT NULL,
 kind TEXT NOT NULL, entity_version INTEGER NOT NULL CHECK(entity_version>0),
 recorded_at TEXT NOT NULL, payload_digest TEXT NOT NULL,
 PRIMARY KEY(project_id,sequence)
);
