import type { PortCapabilities } from "./port";
import type { WorkspaceCommand } from "./types";

/**
 * What a control needs from the machine. Three commands exist on the wire today; every other command names a local
 * capability no service offers yet, so its control stays off until that surface is connected.
 */
const ON_THE_WIRE: Partial<Record<WorkspaceCommand["type"], string>> = { create_task: "tasks.create", start_task: "tasks.start", answer_decision: "decisions.answer" };
export const WIRE_OPERATIONS = Object.values(ON_THE_WIRE);
export const capabilityOf = (type: WorkspaceCommand["type"]) => ON_THE_WIRE[type] ?? `workspace.${type}`;

/** What a machine with its helper offers, past the three commands of the wire. */
export const MACHINE_COMMANDS: readonly WorkspaceCommand["type"][] = [
  "resolve_block", "finish_merge", "abort_merge", "resolve_with_cortex", "stop_resolution", "set_task_settings", "rerun_check", "add_comment", "edit_comment", "remove_comment", "send_comments", "stop_worker", "stop_command",
  "stop_task", "continue_task", "send_message", "remove_queued", "move_queued", "send_queued", "request_changes", "rename_task", "archive_task", "answer_archive", "restore_task", "purge_task", "purge_many", "close_task", "accept_result", "integrate", "set_target",
  "git_stage", "git_revert", "git_draft", "git_commit", "git_fetch", "git_init", "git_pull", "git_push", "create_branch", "revert_commit", "delete_worktree",
  "clear_old", "rename_machine", "set_retention", "save_storage", "set_worktree_root", "run_diagnostics", "run_cleanup",
  "save_surface", "activate_profile", "save_profile", "revert_profile", "deactivate_profile", "create_profile", "rename_profile", "describe_profile", "delete_profile", "apply_preset", "reset_preset",
  "add_connection", "rename_connection", "check_connection", "refresh_models", "remove_connection", "set_sessions", "download_engine", "enable_opencode",
  "create_sprint", "set_sprint_settings", "rename_sprint", "set_run_mode", "edit_plan", "materialize_plan", "cancel_planning", "resume_planning", "run_sprint", "pause_sprint", "answer_planning", "plan_message",
];

/** Surfaces that are not one command. */
export const FEATURES = { pickFolder: "workspace.pick_folder", searchFolders: "workspace.search_folders", terminal: "workspace.terminal", voice: "workspace.voice", attachments: "workspace.attachments", workerMessages: "workspace.worker_messages", taskWorktrees: "workspace.task_worktrees", taskRoute: "workspace.task_route", editFiles: "workspace.edit_files" } as const;

export const noCapabilities = (): PortCapabilities => ({ everything: false, operations: [] });
export const hasCapability = (capabilities: PortCapabilities, capability: string) => capabilities.everything || capabilities.operations.includes(capability);
export const canRun = (capabilities: PortCapabilities, type: WorkspaceCommand["type"]) => hasCapability(capabilities, capabilityOf(type));
