//! How a failed Cortex tool call is reported to the model.
//!
//! Enforcement never changes here; only the report does. Once the channel is
//! authenticated, a handler failure is a structured tool error (`isError`)
//! carrying the stable refusal code, the offending field when known and a short
//! corrective hint, so the model can correct the call. A genuine authority
//! failure (channel no longer current, role/stage not served, protected path,
//! missing owner consent) stays a JSON-RPC refusal, which now names its code.
//! Messages are fixed strings: they never echo another task's data, a path
//! outside the project or a credential.
use cortex_core::CoreError;
use serde_json::{json, Value};

/// Codes that remain authority refusals rather than correctable tool errors.
pub(crate) fn authority(code: &str) -> bool {
    matches!(
        code,
        "channel_authority_refused"
            | "channel_identity_changed"
            | "invalid_channel"
            | "session_mismatch"
            | "stale_worker_authority"
            | "stale_epoch"
            | "tool_refused"
            | "task_outside_sprint"
            | "tool_not_offered"
            | "unknown_session_role"
            | "invalid_native_tool"
            | "worker_required"
            | "writing_worker_required"
            | "writing_scope_required"
            | "readonly_delete_refused"
            | "write_denied"
            | "read_only_task_writer_refused"
            | "outside_cwd_denied"
            | "deletion_not_confirmed"
            | "outside_action_not_confirmed"
            | "skill_link_replace_not_confirmed"
            | "access_mode_changed"
            | "channel_or_access_changed"
    )
}

/// The argument a code is about, when it names exactly one.
fn field(code: &str) -> Option<&'static str> {
    Some(match code {
        "request_key_conflict" | "payload_conflict" | "invalid_request_key" | "invalid_request" => {
            "request_key"
        }
        "invalid_cursor" | "discovery_cursor_stale" => "cursor",
        "runtime_owned_operation" | "invalid_template" => "template",
        "draft_not_found" | "draft_not_owned" | "draft_published" | "draft_conflict" => "draft_id",
        "draft_marker_missing"
        | "draft_guidance_remaining"
        | "invalid_utf8"
        | "document_too_large" => "content",
        "not_found" | "use_read_source" => "report_id",
        "invalid_source_revision" => "source_revision",
        "invalid_scope" | "scope_conflicts_with_user_policy" => "scope",
        "unknown_assignment" => "assignment",
        "gateway_write_conflict" | "file_effect_not_applied" => "expected_sha256",
        "gateway_edit_not_found"
        | "gateway_edit_not_unique"
        | "gateway_edit_no_change"
        | "invalid_edits" => "edits",
        "file_missing"
        | "symlink_refused"
        | "project_mount_alias_refused"
        | "regular_file_required"
        | "regular_unlinked_file_required"
        | "binary_file"
        | "file_too_large"
        | "private_or_invalid_path"
        | "gateway_path_exists"
        | "directory_required"
        | "outside_path_must_be_absolute"
        | "outside_symlink_refused"
        | "outside_path_unavailable"
        | "outside_path_inside_project"
        | "outside_parent_unavailable"
        | "write_outside_assignment_scope"
        | "rule_file_protected" => "path",
        "invalid_cwd" | "outside_cwd_unavailable" | "scoped_command_cwd_refused"
        | "worktree_command_cwd_refused" => "cwd",
        "skill_link_name_invalid" | "skill_source_missing" => "name",
        "skill_link_occupied" | "skill_link_conflict" => "replace_symlink_target",
        "skill_link_collision" | "skill_link_not_managed" | "skill_link_parent_unsafe" => "name",
        "invalid_verification_command"
        | "verification_shell_syntax"
        | "verification_executable_unavailable" => "argv",
        "verification_failed_check_missing" | "automatic_required_check_missing" => "verification",
        "synthesis_text_required" => "text",
        "synthesis_gaps_undisclosed" | "invalid_not_verified" | "synthesis_blocker_required" => {
            "not_verified"
        }
        "invalid_question_batch" | "invalid_question_form" => "questions",
        "unknown_model_hint" => "model",
        "unknown_command_ref" => "tool_call_ref",
        "unknown_large_output_ref" => "large_output_id",
        "unknown_source_ref" => "source_ref",
        "at_revision_out_of_range" => "at_revision",
        _ => return None,
    })
}

/// A short correction for the codes a model can act on.
fn hint(code: &str) -> &'static str {
    match code {
        "owner_file_lease_held" => "The owner is editing this file in Code. Leave it unchanged and report the conflict. Read it again after the lease is released before making a new request.",
        "worktree_command_cwd_refused" => "Omit cwd to use the task checkout, or choose an existing directory inside that checkout. Main and sibling checkouts are not command working directories for this task.",
        "worktree_host_effect_unavailable" => "Host effects and privilege escalation are unavailable for this checkout. Use a contained read-only command or Cortex project file tools; there is no host reroute.",
        "worktree_binding_invalid" | "worktree_unavailable" | "worktree_closed" => "The task's bound checkout is unavailable. Do not substitute the main folder. The owner must recover the bound checkout or start a new task.",
        "worktree_receipt_binding_mismatch" => "This receipt does not describe the task's bound checkout. Use only observations from the current checkout and owner epoch.",
        "managed_codex_host_command_refused" => "Managed Codex cannot run on the host or request privilege escalation. Use project tools or a scratch-only command.",
        "managed_codex_git_refused" => "This Git context tool needs the live repository metadata. Use project reads or retained change evidence; managed Codex commands cannot access the host Git control plane.",
        "scoped_command_cwd_refused" => "Use the current checkout or a directory in it. The command sees a read-only snapshot under /workspace and private scratch under /tmp.",
        "invalid_arguments" => "The arguments do not match this tool's advertised inputSchema. Read its live schema with tools_discover and send only its exact properties.",
        "request_key_conflict" | "payload_conflict" => "This request_key already identifies a different action on this channel. Use a fresh request_key for a different payload, or repeat the identical call to read its retained result.",
        "invalid_request_key" | "invalid_request" => "Supply a request_key of 1 to 128 characters.",
        "invalid_cursor" => "Pass the exact next_cursor returned by the preceding page of this same document, or omit the cursor to start again.",
        "discovery_cursor_stale" => "The tool catalogue changed; start discovery again without a cursor.",
        "runtime_owned_operation" => "The pipeline is runtime-owned. Create an ordinary report with another template.",
        "draft_not_found" | "draft_not_owned" => "Use the exact draft_id of a draft this channel created; list_reports lists own_drafts.",
        "draft_published" => "This draft is already published and immutable. Create a new draft for further changes.",
        "draft_marker_missing" => "Keep the draft's required first line (its Cortex draft ID marker) unchanged.",
        "draft_guidance_remaining" => "Replace every template placeholder and guidance comment before publishing.",
        "not_found" => "Use an exact report_id from list_reports or from a result's report_ref.",
        "use_read_source" => "This document is a captured source, not a report; read it with read_source.",
        "pipeline_missing" => "This task has no pipeline document yet. list_reports lists the task's published reports; read one with its report_id.",
        "sprint_membership_required" => "Sprint reads need a current Workspace sprint member. Continue from the current task context.",
        "sprint_membership_changed" => "The sprint membership changed during this read. Read the current graph again.",
        "tool_not_available" => "That tool is not offered to this channel; tools_discover without a name lists the offered tools.",
        "invalid_scope" => "Each scope entry must be a project-relative path without wildcards, '..' or private runtime storage.",
        "scope_conflicts_with_user_policy" => "A requested path is protected by the owner. Remove it from the request.",
        "replan_limit_requires_user_input" => "The automatic replan limit for this task is reached. Ask the owner through the question route, or report the blocker.",
        "invalid_replan_operation" => "Use replan_request, scope_request or escalation_request with their advertised fields.",
        "direct_answer_not_allowed" | "answer_not_expected" => "A direct answer is accepted only while planning a request that needs no workers. Submit a plan instead, or wait for the runtime.",
        "synthesis_not_expected" => "No synthesis is expected in the current task state; end the turn and wait for the runtime.",
        "synthesis_text_required" => "Supply non-empty text.",
        "synthesis_gaps_undisclosed" => "List each current unresolved check in not_verified using its exact name and a concrete blocker, then submit again.",
        "synthesis_disclosure_in_text" => "Put verification-gap disclosures only in not_verified, not in text.",
        "invalid_not_verified" | "synthesis_blocker_required" => "Each not_verified entry needs the exact check name and a concrete blocker.",
        "plan_not_expected" => "A plan is accepted only while planning; end the turn and wait for the runtime.",
        "owner_question_required" => "A decision only the owner can make blocks this work (capsule obstacles.owner_decisions). Ask it first with user_question_request: one question with concrete options. Plan the blocked work after the answer.",
        "verification_failed_check_missing" | "automatic_required_check_missing" => "Keep the unresolved verification commands in the repair plan with their exact argv. Assign the underlying code fix to a writing worker so the runtime can verify it again.",
        "verification_repair_not_expected" => "No verification repair is expected now; end the turn and wait for the runtime.",
        "unknown_model_hint" => "Use a model from list_route_options, or omit the model hint.",
        "worker_waiting_for_answer" => "A question from this assignment is still open. End the turn; the runtime resumes it with the answer.",
        "invalid_question_batch" | "invalid_question_form" => "Ask 1 to 8 self-contained questions, each with 2 to 8 distinct options.",
        "file_missing" => "The path does not exist. Read the parent directory with project_read and use an existing path.",
        "symlink_refused" => "A path component is a symbolic link; Cortex file tools never follow links.",
        "regular_file_required" | "regular_unlinked_file_required" => "The path must be one regular file (not a directory, link or hard-linked file).",
        "binary_file" => "This operation handles UTF-8 text without NUL bytes only.",
        "file_too_large" => "The file is larger than this tool returns.",
        "gateway_edit_not_found" => "An old_string does not occur in the file as last read. The batch changed nothing. Use project_read with line_numbers=false to get the exact current text, then rebuild the indicated replacement.",
        "gateway_edit_not_unique" => "An old_string occurs more than once. Add surrounding context or set replace_all.",
        "gateway_edit_no_change" => "old_string and new_string are identical; nothing would change.",
        "invalid_edits" => "Each edit needs a non-empty old_string and a new_string.",
        "gateway_write_conflict" => "expected_sha256 must match the file's current bytes (or 'absent' for creation). Read the current file and rebuild the change.",
        "gateway_path_exists" => "A file or link already occupies this path; nothing was created. Choose another directory path.",
        "directory_required" => "The path is a directory; this operation deletes one regular file only.",
        "file_outcome_unknown" | "command_outcome_unknown" | "outside_outcome_unknown" | "project_effect_outcome_unknown" => "An earlier identical request has no settled outcome. Do not repeat it; inspect the current state and report the uncertainty.",
        "command_output_create_failed" => "The private output file could not be created before launch. No command was started by this attempt.",
        "unknown_command_ref" => "Use the exact tool_call_ref returned by command_run on this channel.",
        "invalid_cwd" => "cwd must be an existing absolute directory without '..' components or aliases.",
        "outside_cwd_denied" => "This role, access mode or protected location does not permit that working directory. Choose a directory in the task worktree.",
        "verification_executable_unavailable" => "The program was not found as an executable in the read-only verification environment. For a bare program name, the verification command searches existing project tool directories (.venv/bin, venv/bin, node_modules/.bin), then the owner's validated PATH entries (or system defaults if none are retained).",
        "invalid_verification_command" | "verification_shell_syntax" => "Supply one literal argv: no shell operators, redirections or command substitution.",
        "native_command_sandbox_unavailable" => "The command did not start: its supervisor could not prepare or confirm the sandbox before release. Nothing ran. This refusal does not authorize running on the host. Report the preparation blocker; a fresh contained attempt is appropriate only if the problem has cleared and the original receipt confirms it never launched.",
        "project_mount_alias_refused" => "Use a path directly in the bound checkout; this path crosses a mount boundary.",
        "write_outside_assignment_scope" => "Use a target inside the current assignment scope or request access to an additional project path before retrying. Outside paths cannot be added to the assignment.",
        "rule_file_protected" => "The owner denied writes to this path (`deny-write:`). Leave it unchanged; the owner lifts the denial with `allow-write: <path>`.",
        "outside_action_not_confirmed" => "The user did not allow this to run outside the sandbox, or no one could answer the permission card. Nothing ran. Do not repeat it; report the check as not run with this blocker.",
        "native_verification_sandbox_unavailable" =>"The verification sandbox could not run the probe; this does not show that the program is missing.",
        "skill_link_name_invalid" => "Use the exact skill directory name under .agents/skills: 1-64 lowercase letters, digits and single hyphens.",
        "skill_source_missing" => "Write .agents/skills/<name>/SKILL.md as a regular file in a real directory first.",
        "skill_link_parent_unsafe" => ".claude or .claude/skills is not a real directory; nothing was changed. Report it.",
        "skill_link_collision" => "A file or directory occupies .claude/skills/<name> and is never replaced. Move its content into .agents/skills/<name> and ask the owner to remove it.",
        "skill_link_occupied" => "A different link occupies .claude/skills/<name>. Pass its exact current text (target) as replace_symlink_target to replace it after the owner confirms, or leave it.",
        "skill_link_conflict" => "replace_symlink_target does not match the current link; nothing was replaced. Pass the exact current link text, or omit it.",
        "skill_link_not_managed" => "Only the exact managed link to ../../.agents/skills/<name> is removed. Leave this entry or ask the owner.",
        "capsule_unavailable" => "This worker's own assignment is not recorded yet; retry shortly.",
        "at_revision_out_of_range" => "at_revision must not exceed the task's current source revision.",
        "busy" | "storage_unavailable" | "session_host_closed" | "native_host_closed" => "The runtime store was briefly unavailable; nothing new is known about the outcome. Retry the identical call once, then report it.",
        _ => "No change was made by this refusal unless the code says the outcome is unknown. Correct the call according to the code and the tool's live description.",
    }
}

/// The isError tool result for a handler failure after channel authentication.
pub(crate) fn reply(code: &str) -> Value {
    let mut value = json!({"code":code,"hint":hint(code)});
    if let Some(field) = field(code) {
        value["field"] = json!(field);
    }
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true})
}

/// `reply` naming an exact argument location (for example `calls[2].tool`).
pub(crate) fn reply_at(code: &str, field: &str) -> Value {
    let value = json!({"code":code,"field":field,"hint":hint(code)});
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true})
}
/// The structured error object alone, as one read_batch item carries it.
pub(crate) fn item_error(failed: &Value) -> Value {
    match failed.get("structuredContent") {
        Some(v) if v["code"].is_string() => v.clone(),
        // File read-precondition errors name their code `error`.
        Some(v) if v["error"].is_string() => {
            json!({"code":v["error"],"field":v["field"],"hint":v["correction"]})
        }
        _ => {
            let code = failed["content"][0]["text"]
                .as_str()
                .unwrap_or("tool_error");
            reply(code)["structuredContent"].clone()
        }
    }
}
/// Map a Core failure inside an authenticated tool call: authority codes stay
/// named refusals, every other code becomes a structured tool error.
pub(crate) fn outcome(result: Result<Value, CoreError>) -> cortex_engine::Result<Value> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => finish(Err(cortex_engine::Error::Refused(error.code()))),
    }
}

/// Final mapping for a tool call: a non-authority named refusal is returned to
/// the model as a structured tool error; everything else passes through.
pub(crate) fn finish(result: cortex_engine::Result<Value>) -> cortex_engine::Result<Value> {
    match result {
        Err(cortex_engine::Error::Refused(code)) if !authority(code) => Ok(reply(code)),
        other => other,
    }
}

/// Enrich a bare `isError` text reply (`{"type":"text","text":code}`) with the
/// structured code, field and hint, keeping any already structured reply.
pub(crate) fn enrich(value: Value) -> Value {
    if value["isError"] != true || value.get("structuredContent").is_some() {
        return value;
    }
    match value["content"][0]["text"].as_str() {
        Some(code)
            if !code.is_empty()
                && code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit()) =>
        {
            reply(code)
        }
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_failures_are_tool_errors_and_authority_stays_a_named_refusal() {
        let error = finish(Err(cortex_engine::Error::Refused("request_key_conflict"))).unwrap();
        assert_eq!(error["isError"], true);
        assert_eq!(error["structuredContent"]["code"], "request_key_conflict");
        assert_eq!(error["structuredContent"]["field"], "request_key");
        assert!(matches!(
            finish(Err(cortex_engine::Error::Refused("write_denied"))),
            Err(cortex_engine::Error::Refused("write_denied"))
        ));
        assert!(matches!(
            finish(Err(cortex_engine::Error::Forbidden)),
            Err(cortex_engine::Error::Forbidden)
        ));
        let bare =
            json!({"content":[{"type":"text","text":"gateway_edit_not_found"}],"isError":true});
        assert_eq!(enrich(bare)["structuredContent"]["field"], "edits");
        let ok = json!({"content":[],"isError":false});
        assert_eq!(enrich(ok.clone()), ok);
    }
    #[test]
    fn missing_verification_executable_explains_the_search_path() {
        let reply = reply("verification_executable_unavailable");
        assert_eq!(reply["isError"], true);
        assert_eq!(
            reply["structuredContent"]["code"],
            "verification_executable_unavailable"
        );
        let hint = reply["structuredContent"]["hint"].as_str().unwrap();
        assert!(hint.contains("read-only verification environment"));
        assert!(
            hint.contains("(.venv/bin, venv/bin, node_modules/.bin), then the owner's validated PATH")
        );
    }

    #[test]
    fn an_unprepared_command_sandbox_does_not_authorize_host_execution() {
        let reply = reply("native_command_sandbox_unavailable");
        let hint = reply["structuredContent"]["hint"].as_str().unwrap();
        assert!(hint.contains("did not start") && hint.contains("Nothing ran"));
        assert!(hint.contains("does not authorize running on the host"));
        assert!(hint.contains("original receipt confirms it never launched"));
        assert!(!hint.contains("Correct the call"));
    }
}
