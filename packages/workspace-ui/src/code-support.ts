import type { CodeLeaseAcquireResult, CodeLeaseRenewResult, CodeLspOpenParams, CodeLspOpenResult, CodeLspSnapshot, CodeLspDefinitionParams, CodeLspDefinitionResult, CodeLspEvent } from "@cortex/workspace-contract";
import type { FileSaveResult } from "@cortex/workspace-domain";
import { COPY } from "./copy";

export type CodeFile = { checkoutId: string; path: string };
export class CodeRefusal extends Error {
  constructor(readonly code: string, message: string, readonly holderTaskId?: string) { super(message); }
}
export function leaseMessage(error: unknown, taskTitle?: string) {
  if (error instanceof CodeRefusal) {
    if (error.code === "task_file_lease_held")
      return (taskTitle ?? error.holderTaskId) ? COPY.codeTaskEditing.replace("{task}", () => taskTitle ?? error.holderTaskId!) : COPY.codeTaskEditingUnknown;
    if (error.code === "edit_lease_lost") return COPY.codeLeaseLost;
    if (error.code === "conflict") return COPY.codeDiskChanged;
  }
  return COPY.codeEditUnavailable;
}

export type CodeLeaseIo = {
  acquire(file: CodeFile): Promise<CodeLeaseAcquireResult>;
  renew(file: CodeFile, token: string): Promise<CodeLeaseRenewResult>;
  release(file: CodeFile, token: string): Promise<unknown>;
  save(file: CodeFile, text: string, revision: string | null, token: string): Promise<FileSaveResult>;
};
export type CodeEvent = CodeLspEvent | { type: "code_lsp_resync" };
export type CodeLanguageIo = {
  open(project: string, params: CodeLspOpenParams): Promise<CodeLspOpenResult>;
  change(project: string, documentId: string, text: string, version: number): Promise<CodeLspSnapshot>;
  status(project: string, documentId: string): Promise<CodeLspSnapshot>;
  definition(project: string, params: CodeLspDefinitionParams): Promise<CodeLspDefinitionResult>;
  close(project: string, documentId: string): Promise<unknown>;
  subscribe(listener: (event: CodeEvent) => void): () => void;
};
export type LocalCodePort = { codeLease?: CodeLeaseIo; codeLanguage?: CodeLanguageIo };
export const CODE_LANGUAGE = "workspace.code_language";
