/** The native host offers only these PTY operations, never an executable or cwd. */
export type TerminalPage = { sessionId: string; data: number[]; next: number; truncated: boolean; exited: boolean };
export type TerminalPort = {
  open(projectId: string, cols: number, rows: number): Promise<TerminalPage>;
  read(projectId: string, sessionId: string, after: number): Promise<TerminalPage>;
  write(projectId: string, sessionId: string, data: string): Promise<{ written: number }>;
  resize(projectId: string, sessionId: string, cols: number, rows: number): Promise<unknown>;
  close(projectId: string, sessionId: string): Promise<unknown>;
};
export function createTerminalPort(transport: { request(operation: string, message: unknown): Promise<unknown> }): TerminalPort {
  const page = (value: unknown): TerminalPage => {
    const p = value as TerminalPage;
    if (!p || typeof p.sessionId !== "string" || !Number.isSafeInteger(p.next) || p.next < 0
      || typeof p.truncated !== "boolean" || typeof p.exited !== "boolean"
      || !Array.isArray(p.data) || p.data.length > 16384 || p.data.some(b => !Number.isInteger(b) || b < 0 || b > 255))
      throw new Error("Invalid terminal response. Nothing was resent.");
    return p;
  };
  return {
    async open(projectId, cols, rows) {
      const caps = await transport.request("service.capabilities", {}) as { methods?: string[] };
      if (!["open", "read", "write", "resize", "close"].every(op => caps.methods?.includes(`terminal.${op}`)))
        throw new Error("This service does not offer a terminal.");
      return page(await transport.request("terminal.open", { projectId, cols, rows }));
    },
    async read(projectId, sessionId, after) { return page(await transport.request("terminal.read", { projectId, sessionId, after })); },
    async write(projectId, sessionId, data) {
      const result = await transport.request("terminal.write", { projectId, sessionId, data }) as { written: number };
      if (!Number.isInteger(result?.written) || result.written !== new TextEncoder().encode(data).length)
        throw new Error("Terminal input was only partly accepted. It was not resent.");
      return result;
    },
    resize: (projectId, sessionId, cols, rows) => transport.request("terminal.resize", { projectId, sessionId, cols, rows }),
    close: (projectId, sessionId) => transport.request("terminal.close", { projectId, sessionId }),
  };
}
/** Limit each write by UTF-8 bytes, without splitting a Unicode scalar. */
export function inputChunks(text: string): string[] {
  const result: string[] = [];
  let chunk = "", length = 0;
  for (const char of text) {
    const n = new TextEncoder().encode(char).length;
    if (length + n > 4096) { result.push(chunk); chunk = ""; length = 0; }
    chunk += char; length += n;
  }
  if (chunk) result.push(chunk);
  return result;
}
