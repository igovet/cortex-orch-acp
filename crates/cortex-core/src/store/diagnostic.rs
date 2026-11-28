//! Owner-facing explanations for stable storage refusal reasons.
use crate::CoreError;

/// Value-free storage diagnostic suitable for transport to a project owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageDiagnostic {
    /// The existing machine reason returned by `CoreError::code()`.
    pub reason: &'static str,
    /// What happened and the next safe action, without private paths or error details.
    pub message: &'static str,
    /// Whether this is transient contention that can be retried after waiting.
    pub retryable: bool,
}

impl CoreError {
    /// Explain a storage refusal without changing its machine reason or `Display`.
    /// Non-storage errors return `None`.
    pub fn storage_diagnostic(&self) -> Option<StorageDiagnostic> {
        let reason = self.code();
        let message = match reason {
            "unsupported_storage" if matches!(self, Self::UnsupportedStorageFormat10) => {
                "This project uses format 10 from the retired Python Cortex runtime. This release cannot open or upgrade it. Keep the original files and use a matching historical Cortex archive on a copy, or restore a verified backup in a supported format."
            }
            "unsupported_storage" => {
                "Cortex does not recognize this database as a supported project store. Keep the original files and check which Cortex version created them, or restore a verified compatible backup."
            }
            "legacy_storage_migration_required" => {
                "This format-11 project needs an upgrade before it can be read here. Open it in writable mode with a current Cortex release to run the verified upgrade, which keeps a private backup in .migration-backups/."
            }
            "legacy_storage_root_detected" => {
                "Cortex found older project storage under .codex/cortex/ but no project records in the current store. Nothing was moved. Keep the original files and use a matching historical Cortex release on a copy or recover from a verified compatible backup."
            }
            "partial_storage_schema" => {
                "The project database is missing part of a group of required tables. Keep the original files and restore a verified complete backup or seek recovery help."
            }
            "incomplete_storage_schema" => {
                "A table in the project database is missing required columns. Keep the original files and restore a verified complete backup or seek recovery help."
            }
            "project_storage_mismatch" => {
                "This database belongs to a different project path. Open the project at its original path with its matching store; do not overwrite either store."
            }
            "store_root_mismatch" => {
                "This project is bound to a different Cortex storage root. Use the original channel and state directory to reopen it."
            }
            "project_store_anchor_missing" => {
                "This project is missing its storage anchor. Reopen it in writable mode to restore the anchor for the original project, or seek recovery help."
            }
            "store_root_unsuitable" => {
                "Cortex project history needs a private store on a persistent local filesystem. Check the channel's state directory."
            }
            "store_relocation_conflict" => {
                "Cortex found conflicting copies during store relocation. Keep both copies and retry a writable open to reconcile them."
            }
            "store_return_pending" => {
                "An offline return to the project was interrupted. Keep both copies and repeat the same confirmed project-store command."
            }
            "store_return_conflict" => {
                "Cortex could not verify the identity of the database or files being returned to the project. Keep all copies for recovery."
            }
            "store_relocation_no_space" => {
                "Cortex found too little free space to relocate this project store. Free space on the state filesystem and retry."
            }
            "store_relocation_pending_publication" => {
                "A publication needs reconciliation before Cortex can move this project store. Reopen the original project store and resolve the pending publication first."
            }
            "store_relocation_linked_workspace" => {
                "Cortex found a Git workspace inside the original history directory. Keep the original store and resolve the workspace before moving the history."
            }
            "relocated_store_restore_unsupported" => {
                "This format backup cannot replace a relocated project store. Keep the backup and both storage locations for recovery."
            }
            "busy" => {
                "The project store is busy. Wait for the other Cortex operation to finish, then try again. Close other Cortex sessions for this project if the store stays busy."
            }
            "storage_unavailable" => {
                "Cortex could not access or read the project store. Check that the project's files and disk are available and that your account can access them. If it still cannot be read, keep the files and check a verified backup."
            }
            "unsafe_storage" => {
                "The project store does not meet Cortex's file safety requirements. Check for symlinks, shared files or incorrect ownership or permissions before reopening it."
            }
            "invalid_project" | "project_context_conflict" => {
                "Cortex could not confirm the project path. Open the original project folder, or use a Git worktree with intact metadata."
            }
            "storage_inspection_requires_checkpoint" => {
                "The project database has an active or uncheckpointed SQLite journal. Close writable Cortex sessions normally before retrying offline inspection."
            }
            "storage_replaced" => {
                "The project database was replaced while it was open. Close and reopen the project before continuing."
            }
            "storage_executor_closed" | "storage_executor_unavailable" => {
                "The project storage service is unavailable. Reopen the project; if the problem persists, check the project's files and disk."
            }
            "migration_backup_invalid" => {
                "Cortex could not verify the migration backup. Keep the original store and the backups in .migration-backups/ and seek recovery help before trying again."
            }
            "migration_integrity_failed" => {
                "The database failed integrity checks during migration. Keep the original files and any backups in .migration-backups/ and seek recovery help."
            }
            "migration_requires_stopped_access" => {
                "Offline recovery requires exclusive access. Close all Cortex sessions using this project before trying again."
            }
            "rollback_backup_incompatible" => {
                "The selected backup is not in a supported Cortex storage format. Keep it and choose a verified compatible backup."
            }
            "rollback_requires_confirmation" => {
                "Restoring a backup would replace the current project database. Confirm that you want to restore the verified backup before continuing."
            }
            "inspection_read_only" => {
                "This project is open for read-only inspection. Use a writable Cortex session for changes."
            }
            _ => return None,
        };
        Some(StorageDiagnostic {
            reason,
            message,
            retryable: reason == "busy",
        })
    }
}
