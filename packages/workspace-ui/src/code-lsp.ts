import { CodeRefusal, type CodeLanguageIo } from "./code-support";
import type { CodeLspDiagnostic, CodeLspPosition, CodeLspSnapshot } from "@cortex/workspace-contract";

export const supportsCodeLanguage = (path: string) => /\.(rs|[cm]?[jt]s|[jt]sx)$/.test(path);
export const diagnosticSeverity = (severity: number) => severity === 1 ? "Error" : severity === 2 ? "Warning" : severity === 3 ? "Information" : "Hint";

// CodeMirror and LSP both count UTF-16 code units. Clamp columns but never mark outside the document.
export function diagnosticRanges(diagnostics: readonly CodeLspDiagnostic[], doc: { lines: number; line(n: number): { from: number; to: number } }) {
  const at = ({ line, character }: CodeLspPosition) => {
    const row = doc.line(Math.max(1, Math.min(line + 1, doc.lines)));
    return Math.min(row.to, row.from + character);
  };
  return diagnostics.filter(d => d.range.start.line < doc.lines).map(d => ({
    ...d, from: at(d.range.start), to: Math.max(at(d.range.start), at(d.range.end)),
  })).sort((a, b) => a.from - b.from || a.to - b.to);
}

export function openLanguageDocument(io: CodeLanguageIo, file: { projectId: string; checkoutId: string; path: string }, initial: string,
  publish: (snapshot: CodeLspSnapshot | null, unavailable?: boolean) => void) {
  const { projectId } = file;
  let text = initial, version = 1, sent = 0, generation = -1;
  let handle: { documentId: string; eventId: string } | undefined;
  let disposed = false, openUnavailable = false, timer: ReturnType<typeof setTimeout> | undefined;
  let queue: Promise<unknown> = Promise.resolve();
  const enqueue = <T,>(operation: () => Promise<T>) => {
    const next = queue.then(operation);
    queue = next.catch(() => undefined);
    return next;
  };
  const apply = (snapshot: CodeLspSnapshot) => {
    if (disposed || snapshot.eventId !== handle?.eventId || snapshot.version !== version || snapshot.generation < generation) return;
    generation = snapshot.generation;
    publish(snapshot, snapshot.status === "lsp_unavailable");
  };
  const sync = async () => {
    if (disposed || openUnavailable) return;
    const wanted = version;
    if (!handle) {
      const opened = await io.open(projectId, { checkoutId: file.checkoutId, file: file.path, text, version: wanted });
      if (disposed) { void io.close(projectId, opened.documentId).catch(() => undefined); return; }
      handle = opened;
      generation = -1;
      sent = wanted;
      apply(opened);
      // Catch a publication sent before the open response installed the event identity.
      apply(await io.status(projectId, opened.documentId));
    } else if (sent !== wanted) {
      const snapshot = await io.change(projectId, handle.documentId, text, wanted);
      sent = wanted;
      apply(snapshot);
    }
  };
  const recover = async (operation: () => Promise<void>) => {
    try { await operation(); }
    catch (error) {
      if (disposed) return;
      if (handle && error instanceof CodeRefusal && error.code === "lsp_document_stale") {
        const previous = handle;
        handle = undefined;
        publish(null);
        await io.close(projectId, previous.documentId).catch(() => undefined);
        try { await sync(); return; } catch { /* show unavailable */ }
      }
      if (!handle && error instanceof CodeRefusal && error.code === "lsp_unavailable") openUnavailable = true;
      if (!disposed) publish(null, true);
    }
  };
  const unsubscribe = io.subscribe(event => {
    if (event.type === "code_lsp") {
      if (event.projectId === projectId) apply(event);
    } else {
      if (openUnavailable) return;
      publish(null);
      void enqueue(() => recover(async () => {
        if (disposed) return;
        await sync();
        if (handle && !disposed) apply(await io.status(projectId, handle.documentId));
      }));
    }
  });
  void enqueue(() => recover(sync));
  return {
    changed(next: string) {
      if (disposed || next === text) return;
      text = next;
      version++;
      if (openUnavailable) return;
      publish(null);
      clearTimeout(timer);
      timer = setTimeout(() => void enqueue(() => recover(sync)), 250);
    },
    definition(position: CodeLspPosition) {
      const wanted = version;
      return enqueue(async () => {
        const locate = async () => {
          await sync();
          if (!handle || disposed || wanted !== version) return;
          const result = await io.definition(projectId, { documentId: handle.documentId, version: wanted, position });
          return !disposed && wanted === version && result.version === wanted && result.generation >= generation
            ? result.targets : undefined;
        };
        try { return await locate(); }
        catch (error) {
          if (!disposed && error instanceof CodeRefusal && error.code === "lsp_document_stale") {
            await recover(async () => { throw error; });
            try { return await locate(); } catch { /* show unavailable */ }
          }
          if (!disposed) publish(null, true);
          return undefined;
        }
      });
    },
    dispose() {
      disposed = true;
      clearTimeout(timer);
      unsubscribe();
      if (handle) void io.close(projectId, handle.documentId).catch(() => undefined);
    },
  };
}
