# Security design

**Status: planned security design. The local desktop slice implements the local trust boundary below; its different-OS-user check is unverified.**

Pair a device with an expiring single-use enrollment secret and explicit local confirmation, binding a device key to a limited grant. Keep secrets in OS-protected storage; never log tokens, source contents, prompts or private keys. Revoke immediately, close active sessions and reject future operations. Authenticate both peers independently of relay location. Apply authorization to every request; remote terminal and Git carry real execution authority.

Restrict project paths and defend against traversal, symlink escapes and private-path access. The packaged units root the service at `/`, because a same-user service gains nothing by limiting the owner to their home and owners keep repositories on other disks. A project is a folder the owner can write to, resolved to its real path once at registration and checked as symlink-free on every later use; the channel's private state and runtime directories and any `.cortex` or `.cortex-dev` path stay refused. Human file writes need runtime-confirmed writer quiescence and hash/version checks; a UI pause is not a security boundary. Treat push as an opaque notification hint, not an authority to execute commands.

## Local slice trust boundary (built)

The machine service listens only on a Unix domain socket in a private per-user runtime directory. The directory is mode 0700 and the socket 0600; both ends compare the peer's effective user with their own, and a mismatch is refused. The service opens no TCP, loopback HTTP or WebSocket listener. The actor of every request is derived from the authenticated peer; client-supplied actor values are replaced.

The desktop shell has one main window that loads only bundled local content. Its content security policy allows connections only to Tauri IPC, and it blocks frames, objects, forms and remote scripts. It grants no shell, filesystem or process capability and exposes a closed request command plus a bounded notification command. Native notifications use the fixed session-bus Notify method, a generic title and markup-escaped work label; no question body or executable is accepted. Because the window draws its own frame, the main window alone may also minimize, maximize, close and start dragging itself, and read whether it is maximized; these five permissions act only on that window. External navigation and new windows are refused.

The ACP child runs with a cleared environment. The dev channel points the child's XDG directories at dev-only paths and does not change `HOME` or copy account files. Prompts, transcripts, question text and result bodies are not copied into the public invalidation journal.

The package installs the complete channel-owned runtime, the locked Codex/Claude dependency tree and checksum-verified Node, plus a login-start symlink. Engine discovery prefers the channel's own root-owned bundle. The Codex private patch remains under connection state; the package tree is never patched. Native logins are referenced in place, never copied. See the [licence inventory](../project/bundled-engine-licenses.md) for the owner's 2026-11-06 distribution-risk decision, not Anthropic permission. Native startup uses bounded, fixed `/usr/bin/systemctl --user` reload/start calls; the webview has no arbitrary process interface. Minimal maintainer scripts reload and try-restart active users' matching units on upgrade and stop them on removal, best effort, without changing user data or Paseo. Old `service.env` files cannot redirect the bundled runtime. Bubblewrap remains required; the package does not disable AppArmor restrictions or introduce a privileged sandbox helper.

Machine facts and owner actions run through `machine_tools.py`, a fixed helper the service starts with the same cleared environment as the ACP child. The webview names only an operation from a closed set checked by the bridge and again by the service, a registered project, and parameters. It never supplies a root, a state directory, a command or an executable: the service resolves those. The helper refuses a path that is absolute, contains `..`, starts in `.git`, `.cortex` or `.cortex-dev`, or resolves outside the checkout through a link. An owner save goes further: it refuses `.git`, `.cortex` and `.cortex-dev` at any depth, walks to the file one folder at a time with `O_NOFOLLOW` so no link on the path is followed even inside the checkout, is bounded to 512 KiB of UTF-8 text with no NUL byte, writes only over the revision the owner read, and is refused while the project store does not show every task on that checkout stopped. See [editing files](local-task-execution.md#editing-files). Git runs from fixed argument lists with prompts disabled, a push never forces, and a credential embedded in a remote URL is removed before the URL is reported. Discarding work and deleting need an explicit confirmation in the request. Cortex settings go through the configuration management API with its revision checks, so a stored MCP secret is never read back. The project store is opened read-only. The separate owner terminal boundary below intentionally grants interactive shell authority; it is not routed through this helper.

### Local dictation (built, local Linux)

Only bundled main-window content may call voice.start/read/stop/cancel; the
same-user socket and shared Rust parser enforce the closed identities and refuse
audio, file paths, URLs, commands, environments and send flags. A random session
identity binds reads and controls; recent start identities do not restart capture.
There is one microphone lease, refreshed by reads and cancelled after ten seconds
without a read. Stopping finalizes a preview and cannot send anything.

The service starts its fixed packaged worker with a cleared environment, retaining
only HOME, LANG and the actual desktop XDG_RUNTIME_DIR for local audio access.
It uses the bundled Node and fixed parec capture arguments. No project, engine,
credential environment or webview-selected executable is passed. The helper
downloads only the committed model URL, checks the exact size and SHA-256 before
extracting named model members, verifies their hashes and caches them under the
channel's private state directory. Audio stays in bounded process memory; transcript
snapshots are local IPC only, never public journal/status logs. A recording stops
at 90 seconds. Cancel, service stop and lease expiry kill the owned worker/capture
process group and reap the worker. Abrupt service death has no guarantee of reaping
descendants beyond the helper's parent-death signal; same-user code is trusted.
This is local microphone authority, not hostile-code isolation.

### Owner terminal (built, local Linux)

Ctrl+J opens one service-owned PTY per registered project, including from its task views. The displayed project folder is the initial cwd, not a task worktree. The service selects the user's executable shell (native SHELL/password-database fallback), passes only the fixed interactive flag and TERM=xterm-256color, and runs as the same OS user with that user's service environment. It does not activate Cortex or a model. The webview cannot choose an executable, cwd, environment, command argument list or one-shot execution operation.

Only bundled main-window content can call terminal.open, terminal.read (output only), terminal.write, terminal.resize and terminal.close through the existing same-user Unix socket. The Rust request types reject unknown fields and invalid identities, dimensions or sizes in both the native bridge and service. Every request binds a registered project; subsequent operations also bind its unpredictable session identity. Unknown/removed projects and lost sessions are refused. Opening an existing project PTY reattaches, never restarts a command. The initial directory is rechecked, including dev-store ownership and canonical-path equality.

This is deliberate **owner shell authority**, not a sandbox: typed shell input can execute programs, change cwd, access the user's files/accounts and network, or change production data. A compromised trusted webview has that same terminal authority. The closed operations prevent a second generic process API, not shell execution through input. Terminal bytes never enter the public journal or service status log; only a bounded in-memory tail is retained.

There are at most eight sessions (one per project), 64 KiB retained output per PTY, 16 KiB per output page, 4 KiB per input write and a 16 KiB client input queue. Dimensions are 2–500 columns and 1–500 rows; browser scrollback is 1,000 lines. Nonblocking master IO and bounded drain work prevent slow input/output from creating unbounded service queues. A dropped tail is explicit. Short writes and uncertain deliveries are not replayed. The UI stops input on failure and offers explicit reattachment; it never resends prior input.

Hiding or closing the window detaches only. Explicit Close shell, project removal and orderly service stop terminate the PTY's dedicated session process groups: SIGTERM, 150 ms grace, then SIGKILL, followed by shell reaping. Foreground and background job groups are included. The shell PID is retained until cleanup to prevent identity reuse. A service restart has no terminal history or command replay. Abrupt service death, intentionally daemonized jobs that leave the session, shell resource consumption and hostile same-user code are not contained by this cleanup mechanism; there is no CPU, memory, filesystem or network sandbox for the owner's shell. Remote terminals, task/worktree-specific PTYs and GPU addons remain unimplemented; the pinned xterm.js 6.0.0 DOM renderer and fit addon 0.11.0 use theme tokens and the bundled monospace font.

Dev uses `.cortex-dev` beside release/default `.cortex`, without opening, importing or migrating release data. Registration reserves `.cortex-dev-owner.json`; activation binds the exclusively created dev directory by device/inode. An unowned or changed dev store is refused, not a folder merely containing release data. Older markers/data are left untouched; a project an older build activated is reserved at activation only while no `.cortex-dev` exists. One store per logical project per channel is shared by linked worktrees ([CW-S12](backlog/CW-S12.md)).

Limits: the same OS user is local trust, not isolation from hostile code running as that user. The different-OS-user connection check is not yet verified. The push gateway runs as a self-hosted container on the owner's server ([ADR 0011](adr/0011-self-hosted-relay-and-push-gateway.md)). It holds the FCM credential as a Docker secret file, authenticates desktop callers, rate-limits them and rejects any payload with fields beyond the closed schema. Its logs must contain no task content, payload body or full token. The FCM credential and caller-authentication keys stay outside git and are absent from client builds. The existing [ACP boundary](../../SECURITY.md) remains authoritative for current behavior.

Server-side trust boundary: the owner's server runs the relay and the gateway, and it is internet-reachable. Its compromise would expose the FCM credential, allow abuse of the gateway, and expose relay metadata. The relay can be given an access policy (allowlist, shared token or HTTP callout), but device authorization still rests on the desktop (CW-N04) and never on the relay. Threat-model relay compromise, gateway compromise, server compromise, secret-file leakage, stolen devices, key rotation and malicious projects before implementation.
