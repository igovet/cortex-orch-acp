# Privacy

Cortex stores original user requests, advisory governance, task metadata, and authored Markdown locally in each project's private home state store. The release location defaults to `~/.local/state/cortex/projects/`; Workspace Dev uses its separate state root. The published documents, SQLite index and newest-first `pipeline.md` live together. The checkout holds a location marker that contains no task history. See [storage locations](docs/project/storage.md#where-the-database-lives-and-how-it-moves).

The server temporarily creates typed report and pipeline drafts in the project's owner-private `.cortex/draft-reports/` and `.cortex/pipeline-drafts/` directories, bound to the calling native thread. Agents fill only the returned file. A successful write removes its source after the immutable task file and metadata commit; a failed write leaves it for an exact retry or explicit retention cleanup.

No network service or telemetry endpoint is part of the runtime. Optional local development observations contain operation outcomes and package identity plus native thread/parent identifiers needed to verify task routing, not report bodies, tool arguments, or other transport metadata. Thread bindings are retained locally with task metadata and deleted by task retention cleanup. The host and selected AI provider may have their own data policies. Keep credentials and unnecessary personal data out of report bodies, metadata, and shared diagnostics. See [SECURITY.md](SECURITY.md).

The desktop and Android apps keep unsent conversation and question text in
the current app profile's local WebView storage. A phone separates each
pairing. The feature stores the text, a local version and identifiers that bind
it to the conversation or question. It does not store attachments, selected
options, password or token form inputs, and does not sync drafts to the machine,
another device or an AI provider. The text is visible to anyone who can access
that app profile. Use Discard text draft or Discard answer draft to remove the
current text. Android app data is excluded from backup.
