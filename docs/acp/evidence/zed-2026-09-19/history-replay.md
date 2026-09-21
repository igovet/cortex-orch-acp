# Structured history replay in isolated Zed (2026-09-19)

Client: Zed 1.20.2 (`7c451e694f3c52ee0aeb01d7e28b5fa18cd0ad2f`),
visible on the test desktop. The Zed profile and project were disposable; no
production Zed settings were changed. This was a no-paid probe through the
actual Cortex ACP entry point with a deterministic in-process runtime. It did
not invoke Codex, Claude, or a real delegated worker.

The test runtime emitted, in order, an agent text chunk, a worker `tool_call`,
a fixture-read `tool_call`, two `tool_call_update` completions, and a final
agent text chunk. Before restart, Zed displayed the worker and fixture-read
cards between the two text chunks: [cropped observation](history-before-restart.png).
The project-local `.cortex/cortex.sqlite3` contained three transcript rows
(one user message and two agent text chunks) and four structured host update
rows (two `tool_call`, two `tool_call_update`).

The isolated Zed editor was terminated and relaunched with the same test
profile. The same project was reopened, causing the saved ACP session to load.
Zed again displayed both cards in their original order between the text
chunks: [cropped observation after restart](history-after-restart.png). The
screenshots are cropped to the conversation and contain no credentials. This
qualifies **display replay of newly recorded text and structured ACP updates**
in this Zed version. It does not qualify a paid engine turn, worker execution,
or completion of the earlier failed repair scenario.

Two earlier test tasks (`t_202a8bb695af`, `t_77acef570693`) were created
before structured update recording. Their durable `host_updates` counts are
zero, so the exact cards previously shown for them cannot be reconstructed
from the stored text or runtime task observations. Those old sessions remain
incomplete historical evidence; this result does not mark them repaired.
