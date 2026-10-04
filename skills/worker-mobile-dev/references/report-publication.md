# Report publication

For a separately published report, create a draft through the `cortex` MCP
server's advertised draft tool and preserve its registered first line and exact
path. For an inline `result_submit` report, supply its title and complete Markdown
without a draft marker. Fill either report with the work performed, changes,
findings, actual checks, risks and artifact references. Keep absent evidence
explicit; prose cannot manufacture a successful observation. Every unrun or
unverified check in the report names its concrete blocker.

Use the current advertised Cortex MCP draft, publication and `result_submit`
operations and their exact receipts. Either submit the finished report inline
with the structured result in one call, which publishes it as your immutable report,
or publish your draft first and submit the result with the exact returned
report id. An uncertain delivery is
retried only according to the operation's idempotency contract. A later attempt publishes a new report rather
than rewriting a published result.

Before the first submission, preflight all required and conditional requirements
in the current tool declaration, including publication evidence. A conclusion or
verdict alone is not the immutable report. Pass structured arguments rather than
hand-concatenating report prose into JSON. After a JSON parse failure, fix the
serialization before retrying; if inline serialization repeatedly fails, use the
advertised owned-draft publication route and its acknowledged report identity.
This is transport guidance, not permission to omit evidence or relax validation.

Runtime state controls review, verification and completion. A final chat message
is visible communication and does not replace the structured result.
