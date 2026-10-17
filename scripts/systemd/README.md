# Opt-in native CLI auth keepalive: systemd user templates

These two files are **templates**. Nothing in this repository installs,
enables or starts them, and applying them is an explicit owner action on
the owner's own machine, not part of any Cortex task or runtime behavior.

- `cortex-auth-keepalive.service` — a `oneshot` unit that runs
  `scripts/cortex_auth_keepalive.py`.
- `cortex-auth-keepalive.timer` — runs that service roughly every 4 hours
  (15 minutes after boot, then every `OnUnitActiveSec`).

As of this writing, `scripts/cortex_auth_keepalive.py` reports both the
Claude Code and Codex CLI engines as `unsupported`: installed-CLI `--help`
text describes status/login display only, never a proven OAuth refresh or
persistence of either engine's native login. Installing this
timer today exercises the sanitized check/report plumbing on a schedule; it
does not yet keep any session alive. Re-check `scripts/cortex_auth_keepalive.py`
after a future investigation records a proven command for an engine.

## Manual install

1. Copy both files into `~/.config/systemd/user/`:

   ```
   mkdir -p ~/.config/systemd/user
   cp scripts/systemd/cortex-auth-keepalive.service scripts/systemd/cortex-auth-keepalive.timer ~/.config/systemd/user/
   ```

2. In the copied `.service` file, replace `__CORTEX_CHECKOUT__` with the
   absolute path to this checkout (the directory containing this `scripts/`
   directory), for example with:

   ```
   sed -i "s#__CORTEX_CHECKOUT__#$(pwd)#" ~/.config/systemd/user/cortex-auth-keepalive.service
   ```

3. Reload user units and enable the timer (this step genuinely installs and
   starts a scheduled job — run it only when you want that):

   ```
   systemctl --user daemon-reload
   systemctl --user enable --now cortex-auth-keepalive.timer
   ```

4. Check status and recent runs:

   ```
   systemctl --user status cortex-auth-keepalive.timer
   journalctl --user -u cortex-auth-keepalive.service
   ```

## Manual uninstall

```
systemctl --user disable --now cortex-auth-keepalive.timer
rm ~/.config/systemd/user/cortex-auth-keepalive.service ~/.config/systemd/user/cortex-auth-keepalive.timer
systemctl --user daemon-reload
```

## What the service does and does not do

- It runs `cortex_auth_keepalive.py` with the owner's own environment
  (`HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`), so it reads and can refresh
  the owner's real native CLI config in place; it never copies credentials
  elsewhere.
- It never prints or persists a token value or raw command output; the
  script discards that output and reports only a sanitized
  engine/status/exit_code/detail JSON line per run, visible with
  `journalctl`.
- A non-zero exit (visible in `systemctl status`/`journalctl`) means a
  configured engine's check command failed or needs a manual
  `claude /login` or `codex login`; it never triggers another repair action
  automatically.
- It is independent of the Cortex runtime's own one-shot native-auth-loss
  process restart (see
  [authentication recovery](../../docs/features/failures-and-cancellation/index.md#native-authentication-loss)):
  that restart happens inside an active Cortex session; this timer is an
  unrelated, manually-scheduled background check that does not start or
  touch any Cortex process.
