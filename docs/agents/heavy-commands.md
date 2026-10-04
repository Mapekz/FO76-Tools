# Heavy local commands

Use the guarded launcher for potentially heavy local ESM operations and Rust
builds, clippy or tests. An uncapped ESM diff can exhaust the desktop's memory.
Temporary files on a RAM-backed filesystem also consume that memory.

From the repository root:

```sh
just heavy cargo just esm::check
just heavy cargo cargo build --release -p esm
just heavy esm esm diff --help
```

From another working directory, invoke the executable
[tools/run-heavy.sh](../../tools/run-heavy.sh) with the profile followed by the
command and its arguments. The launcher preserves arguments and the caller's
working directory and stdin.

The cargo profile applies a 10G hard memory cap; ESM applies 5G. Both disable
swap and apply the cap to all descendants through a systemd user scope. Before
executing the command, the launcher checks the actual cgroup limits and rejects
missing or ineffective memory and lifecycle controls. A stable scope name allows one heavy
command per user across working directories and worktrees, until every process
in the scope has exited. An already active scope returns 75 immediately;
simultaneous startups also fail rather than overlap. A command exceeding its
cap can fail; scope OOM handling terminates the command tree rather than growing
into the desktop's memory. Measure a smaller case before increasing workload.

The launcher requires Linux with an available systemd user manager and the
systemd, util-linux and coreutils tools. It never retries without caps. Existing CI check
recipes remain usable on isolated runners without these local prerequisites.

Scratch defaults to the user's XDG cache directory under fo76-tools/heavy. Set
FO76_SCRATCH_DIR to another disk directory when needed. RAM-backed tmpfs and
ramfs scratch is rejected. TMPDIR, TMP and TEMP point at a private per-run
directory using absolute paths, even if FO76_SCRATCH_DIR is relative. Invalid
relative XDG cache settings use the standard home-directory default. Scope
exclusion belongs to the user manager and is independent of XDG state settings.
The temporary directory is removed after all owned scope processes finish. The full output log stays on disk
for inspection and can be removed after reviewing it.

The launcher prints the scope name, memory cap and log path, then the exit
status. Failures include only the last 30 lines, further limited to 4000 bytes.
Read the saved log with a targeted search or parser for details. Successful
commands return the log path without flooding the conversation. The original
command exit status is preserved.

Interrupting the launcher with Ctrl-C, TERM or HUP cancels pending startup and
forcefully terminates its original cgroup, including TERM-resistant descendants.
It opens and verifies the original kernel object before acting, so a newer job
reusing the scope name is unaffected. If the launcher was killed forcibly, use the printed scope name
with `systemctl --user stop` to stop that specific command. Its memory cap and
lock still protect against another overlapping heavy job while descendants
remain alive. Its temporary directory is retained; remove the printed run
directory only after that scope is inactive and the log has been reviewed.
Do not kill processes by a name or path pattern.

Run `just heavy-test` to verify the real cgroup limits, argument handling,
failure output, shared scope, surviving descendants, startup cancellation,
stdin and disk enforcement without game
data or large allocations. Those integration tests explicitly skip when a
systemd user manager is unavailable. Test fixtures use distinct scope names so
their tiny probes do not interfere with the user's production heavy scope.
