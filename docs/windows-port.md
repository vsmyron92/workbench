# Porting the server to Windows

**Status: planned, not started.** Workbench's server is Unix-only today: `nix` and `libc`
are unconditional dependencies and nothing is behind `cfg(windows)`. This is the plan for a
native `x86_64-pc-windows-msvc` build that works on Windows 10 and 11, with Linux behaviour
unchanged. File and line references are from 0.1.0 (commit `493a66e`) and will drift.

Estimated size: 6–8 engineer-weeks, in 14 steps that each compile and pass on Linux.

## Core idea

Every OS-specific site goes through one new core module, `server/src/util/os/`
(`mod.rs`, `unix.rs`, `windows.rs`), with the areas `perm`, `fs`, `proc`, `session`,
`shell`, `exe`, `path`, `net` and `desktop`. The Unix bodies are today's code, moved
verbatim from the call sites, so Linux behaviour stays identical by construction.
Windows-only behaviour is always `cfg(windows)`.

## 1. Inventory and abstractions

**A. Dependencies (`Cargo.toml`)**

- `nix` (signal, process, term, fs, user) and `libc` move to `[target.'cfg(unix)'.dependencies]`.
- `[target.'cfg(windows)'.dependencies]`:
  - `windows-sys = "0.61"` (0.61.2 is already in Cargo.lock through tokio, mio, socket2 and
    keyring). Features: Win32_Foundation, Win32_Security (+ Authorization),
    Win32_Storage_FileSystem, Win32_System_{JobObjects, Threading, Console, RestartManager,
    Registry, Diagnostics_Debug}, Win32_NetworkManagement_IpHelper, Win32_Networking_WinSock,
    Win32_UI_Shell.
  - `sysinfo` (default features off, `system`), for process lists; confirm the version with
    `cargo add`.
  - `mslnk` for the Start Menu shortcut; `dunce` (already in the lock).
- Windows dev-dependency `junction`; build-dependency `winresource` (icon and version
  resource, a no-op elsewhere).
- Not needed: `if-addrs`, `trash`, `winreg`, `windows` (each replacement is under 80 lines of
  windows-sys).
- Code that does not compile on Windows: `tokio::process::Command::{process_group,
  pre_exec}`; portable-pty's `MasterPty::as_raw_fd` (used at `terminals/pty.rs:769`).
  `keyring` 4 already uses Windows Credential Manager.

**B. Private files and modes → `os::perm`**

- The helpers in `util/fs.rs:14-55` (`write_atomic(path, data, mode)`, `set_mode`) have about
  40 callers passing 0o600, 0o700 or 0o644: the token (`auth.rs:173-177`), config
  (`config/mod.rs:37`, `config/global.rs:284`), terminals (`mod.rs:1318`, `store.rs:61-71`,
  `routes.rs:271`, `agent.rs:380-381, 835-836`), `lsp/trust.rs:67`, push
  (`vapid.rs:71,87,144`, `mod.rs:502,914`), `devcontainer/store.rs:50-61`, workspace
  (`store.rs:118,159,669,1215`, `watch.rs:71`), git (`rebase_i.rs:447-455`, `shelf.rs:330`,
  `askpass.rs:122`), `apps/expand.rs:310`, `db/mod.rs:182,200`.
- Direct `PermissionsExt` / `OpenOptionsExt` / `DirBuilderExt` sites: `projects.rs:267-270`,
  `secrets.rs:114-116`, `platform/settings.rs:609, 732-735, 832-833`, `db/conn.rs:192-195`,
  `debug/session.rs:905`, `debug/derive.rs:366-386` and `lsp/config.rs:527-529` (execute bit),
  `files/content.rs:460-466`, `files/ops.rs:206, 215`, `files/history/store.rs:175, 373, 389,
  678`, `files/trash.rs:48-65`, `workspace/store.rs:291-299, 922-926`.
- API: `apply(path, mode)`, `create_dir_private`, `open_new(path, mode, nofollow)`,
  `privacy(path) -> Private | Exposed(why)`, `owned_by_me`, `is_executable`,
  `copy_permissions`.
- Windows:
  - A mode with `mode & 0o077 == 0` becomes a protected DACL granting only the current user's
    SID and SYSTEM, inherited by children for directories, so everything under the data dir
    is born private. It is set on the handle before any byte is written. Other modes inherit
    the parent's ACL.
  - `privacy` reads the DACL and reports read access for anyone but the owner, SYSTEM and
    Administrators.
  - `is_executable` checks PATHEXT; `copy_permissions` does nothing.
  - `write_atomic` copies an existing file's DACL to the temp file and retries the rename up
    to 10 times, 50 ms apart, on sharing violations (Defender, editors).

**C. Rename and symlink primitives → `os::fs`**

- Sites: `files/ops.rs:9, 168-199` (`renameat2` with RENAME_NOREPLACE, EXDEV, symlink copy);
  `workspace/store.rs:298, 329, 360-392, 925` (RENAME_EXCHANGE, O_NOFOLLOW, OsStrExt);
  `workspace/trash.rs:246-251`.
- `rename_noreplace` → `MoveFileExW(from, to, 0)`. `rename_exchange` → Unsupported, so the
  existing fallback runs. NOFOLLOW → `FILE_FLAG_OPEN_REPARSE_POINT`. Symlink copies use
  `symlink_file` or `symlink_dir` by target. A case-only rename (`a.txt` → `A.txt`) must not
  be refused as "exists": check `same_file` first.

**D. User and device identity**

- Sites: `settings.rs:732` (uid), `debug/procs.rs:87-102`, `files/trash.rs:29, 41-131` (dev,
  uid, sticky bit), `config/global.rs:290` (`/tmp/claude-<uid>`), `devcontainer/ops.rs:156-157`.
- Windows: `owned_by_me`; sysinfo's user for the process list; the Recycle Bin (see L); the
  default extra root becomes Claude Code's Windows temp directory (to be verified).

**E. Process trees and signals → `os::proc`**

- Sites: `lsp/server.rs:157, 207, 450-476, 538`; `debug/process.rs:196, 237-262`;
  `debug/session.rs:1329-1332, 1766, 1794`; `git/remote.rs:275-284, 326-331` (`setsid`);
  `files/history/mod.rs:459-466`; `util/mod.rs:10-23` (SIGTERM).
- API: `ProcGroup::{prepare(&mut Command), attach(&Child), terminate, kill, members}`,
  `kill_pid`, `pid_alive`, `parent_of`, `exit_text(ExitStatus)`, `detach_console`,
  `shutdown_signal`.
- Windows: children start with `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` and join a Job
  Object with `KILL_ON_JOB_CLOSE` right after spawn; `terminate` and `kill` both call
  `TerminateJobObject` (the graceful step is the protocol one that already runs first: LSP
  shutdown/exit, DAP disconnect). `shutdown_signal` listens to
  `tokio::signal::windows::{ctrl_c, ctrl_break, ctrl_close}` and a named event
  `Local\workbench-<sha(data_dir)>` that `workbench service stop` and the launcher set.

**F. PTYs → `os::session`**

- `terminals/pty.rs`: 643-648 (`poll`), 769-773 and 806-809 (`dup`/`close` for the redaction
  hold-back), 857-860 (resize), 870-994 (`kill_session`, `session_members`, `processes`,
  `signal_session`, `pid_alive`). `terminals/mod.rs`: 863, 1401, 1463 (lingering processes);
  1136, 1175, 1236-1241 (`$SHELL -l`).
- Windows: a terminal's session is a Job registered under the leader pid, so the existing
  `i32 sid` keys keep working. Hang-up is `ClosePseudoConsole` (CTRL_CLOSE_EVENT to every
  attached process), then `TerminateJobObject` after the grace period. The redaction hold-back
  uses a reader thread feeding a channel with `recv_timeout(HOLD_BACK)`.

**G. `/proc` introspection**

- Sites: `pty.rs:902-964` (`cli_running_in`: cwd and cmdline of other processes);
  `codex.rs:366-371, 453-470` (`/proc/pid/fd`); `debug/procs.rs:38, 80, 87-147` and
  `debug/routes.rs:113` (attach list, `ptrace_scope`); `debug/session.rs:913-924`
  (`TracerPid`); `agent.rs:245-260`, `git/mod.rs:113`, `askpass.rs:117` (the "` (deleted)`"
  executable path); `service.rs:282`; `app.rs:167` (`bindv6only`).
- Windows: sysinfo for the process list, cwd, cmdline, parent and start time; the Restart
  Manager (`RmGetList`) for "who holds this rollout file open"; `CheckRemoteDebuggerPresent`
  for `TracerPid`; `ptrace_scope` is `None`. No " (deleted)" fallback: a running exe cannot be
  replaced on Windows.

**H. Shells → `os::shell`**

- Host-side sites: `terminals/mod.rs:1236-1241`; `apps/runs.rs:798, 1218, 1248, 1348`
  (`bash -lc`); `apps/remote.rs:102`; `debug/session.rs:999`; `platform/notify.rs:222-229`
  (`sh -c`); `devcontainer/ops.rs:193`. POSIX quoting: `apps/expand.rs:185 shell_quote` (47
  callers building `bash -lc` commands), `terminals/input.rs:69-75 quote_path`,
  `agent.rs:262-268, 821` (the statusline command). `/bin/sh` inside containers stays.
- API: `interactive()`, `run_argv(cmd)`, `quote(s)`, `helper_command(exe, args)`.

**I. Finding programs → `os::exe`**

- Sites: `util/mod.rs:54-62 which_path` (no PATHEXT; only `/` makes a path);
  `agent.rs:271-288 resolve_command`; `apps/runs.rs:372-384`; `lsp/config.rs:489-542`
  (execute bit; rustup proxies are detected by canonicalize, which fails on Windows where the
  proxies are hard links); `debug/adapters.rs:173` (`python3`); `devcontainer/mod.rs:187`
  (`npx`).
- Windows: `resolve()` returns `{program, prefix_args, kind: Exe | NpmShim | Batch}` over
  PATH × PATHEXT plus `%USERPROFILE%\.local\bin` and `%APPDATA%\npm`; a rustup proxy is
  `same_file` as `~/.cargo/bin/rustup.exe`; Python is `python`, then `py -3`.

**J. Paths → `os::path` and `util/paths.rs`**

- "Is absolute" written as `starts_with('/')`: `terminals/mod.rs:1247`, `apps/runs.rs:390`,
  `agent.rs:347, 2313` (the hooks' `transcript_path`), `debug/launch.rs:211, 254, 432, 609`,
  `debug/session.rs:456`, `debug/routes.rs:443`, `workspace/store.rs:837, 874`,
  `permission.rs:282`, `gemini.rs:133`. `~/` expansion: `config/mod.rs:60-78`. LSP URIs:
  `lsp/uri.rs:47-62, 112-114, 143-176, 232-252`. Claude's transcript folder name:
  `transcript.rs:30`.
- Every validator that splits on `/` alone (for example `debug/breakpoints.rs:88-105`) must
  also reject `\` and `X:` on Windows, or `..\..\x` and `C:\x` escape the project through
  `root.join()`.

**K. Networking**

- `netif.rs:13-46` (`getifaddrs`) → `GetAdaptersAddresses` with the adapter's friendly name;
  add `vEthernet`, `VMware`, `VirtualBox` to the virtual-interface list (`netif.rs:71`).
- `app.rs:160-170`: `[::]` is v6-only on Windows unless asked, so `os::net::bind` clears
  `IPV6_V6ONLY` for it: one dual-stack socket takes IPv4 (loopback included) as on Linux,
  and TLS covers both families.
- `apps/runs.rs:480-497` (`fuser -k`) → `GetExtendedTcpTable` for the owning pid; only the
  same user's processes are terminated.

**L. Desktop integration**

- `util/mod.rs:27-47 open_in_browser`: Edge or Chrome from the registry's App Paths with
  `--app=`, else `ShellExecuteW`; never `cmd /c start` (cmd interprets `&` in a URL).
- Trash (`files/trash.rs`, `git/ops.rs:182-185`) → `SHFileOperationW(FO_DELETE,
  FOF_ALLOWUNDO | …)`.
- `notify.rs:198` (`notify-send`) reports `desktop: "unavailable"` in the first version.

**M. Service:** `platform/service.rs` (systemd unit, `.desktop` file, `systemctl`) gets a
sibling `platform/service_windows.rs` (section 2).

**N. Git helpers:** `askpass.rs:112-125` writes a `#!/bin/sh` wrapper; `remote.rs:314,
326-331`; `git/mod.rs:103-117` and `rebase_i.rs:465-481` build the editor prefix with
`sh_quote`. The diff reads the committed side with `cat-file blob` and the working tree as
raw bytes (`git/diff.rs:372-416`); line staging uses `git apply --cached` (`git/lines.rs:363`).
Both break under `core.autocrlf` (section 2).

**O. Directories**

- `config/mod.rs:26-39`: `dirs::config_dir()` and `dirs::data_dir()` are both `%APPDATA%` on
  Windows. The data dir moves to `%LOCALAPPDATA%\workbench`, so tokens don't roam and the
  config watcher does not see data writes.
- `claude_mcp.rs:92`: `/etc/claude-code` → `C:\Program Files\ClaudeCode`.
- `db/conn.rs:161`: `~/.pgpass` → `%APPDATA%\postgresql\pgpass.conf`, without the mode check
  (as libpq does on Windows).

## 2. Behaviour that needs care

**Paths.** Canonicalize with `dunce` so no `\\?\` reaches `project-ids.json`, transcript slugs
or comparisons; uppercase drive letters; compare prefixes case-insensitively in
`resolve_absolute_in`; `resolve_in_root` also rejects `:` inside a component (alternate data
streams), reserved device names (`CON`, `NUL`, `COM1`…, also with an extension) and
components ending in a dot or space. UNC roots (including `\\wsl$`) are refused with a clear
message. Test that the canonicalize check follows junctions. `expand_tilde` accepts `~\`.

**LSP URIs.** Emit `file:///C:/…`; accept `/c:/` and `/C%3A/`; match the project root with a
case-insensitive drive letter (servers often lowercase it); `lsp-src://pid/C:/…`.

**Line endings.** Add `.gitattributes` (`* text=auto eol=lf`, `*.ps1` and `*.cmd`
`eol=crlf`). In the git slice read `git ls-files --eol <path>`: when the index has LF and the
working tree CRLF, strip `\r` from the working-tree side for the diff and for the patch given
to `git apply --cached`, and put CRLF back when rolling lines back into the working tree.

**Program lookup, `.cmd` shims and BatBadBut.** portable-pty resolves PATHEXT and launches
`claude.cmd` with MSVCRT quoting (`cmdbuilder.rs:581-606, 702`): command injection through
cmd.exe for an agent prompt passed in argv. So Workbench always hands portable-pty an
absolute path; npm shims (codex, gemini, typescript-language-server, pyright…) are unwrapped
to `node.exe <package script>` (parsed from the `.ps1` next to the shim), which also avoids
cmd's "Terminate batch job (Y/N)?" and its current-directory search for `node`. Real
`.bat`/`.cmd` files run only when their arguments contain none of `%!^&|<>"` or newlines;
otherwise the prompt is pasted instead. Prefer a native `claude.exe` in
`%USERPROFILE%\.local\bin`. Set `NoDefaultCurrentDirectoryInExePath=1` for non-interactive
shells Workbench starts.

**Shells.** Terminals: `pwsh.exe -NoLogo`, then `powershell.exe -NoLogo`, configurable in
`[terminals] shell`. Runs, pre-launch steps and the notify command: `pwsh -NoLogo -NoProfile
-EncodedCommand <base64 UTF-16LE>`, which survives portable-pty's quoting. `run_shell = "cmd"
| "powershell" | "bash"` selects cmd, PowerShell 5.1 (no `&&`) or Git Bash, shown in the
run's argv; `quote()` follows the choice. Add `WT_SESSION` and `WT_PROFILE_ID` to
`PARENT_TERMINAL_VARS`.

**Process trees.** Job Objects replace process groups and the `/proc` session scan;
`TerminalInfo.lingering` is the job's process count minus one. `KILL_ON_JOB_CLOSE` matches
Linux, where closing the PTY hangs up its processes.

**Signals.** `\x03` typed in a terminal becomes CTRL_C_EVENT through ConPTY. Non-PTY children
get a hidden console of their own, so the server's Ctrl-C never reaches them (the
counterpart of `process_group(0)`). `ExitInfo.signal` is always `None`.

**Terminals (ConPTY).** Resize is `ResizePseudoConsole`. ConPTY gives no EOF when the child
exits: close the pseudoconsole once the leader has exited and the job is empty, on a blocking
thread while the reader keeps draining. portable-pty creates the console with
`INHERIT_CURSOR`, so ConPTY sends `ESC[6n` and waits; the existing headless DSR answer
covers it (add a test). Ship a side-loaded `conpty.dll` and `OpenConsole.exe` (the
Microsoft.Windows.Console.ConPTY package, MIT), which portable-pty loads from the exe's
folder; the inbox ConPTY renders poorly on Windows 10.

**Git.** `GIT_ASKPASS` is the absolute `workbench.exe` with `WORKBENCH_HELPER=askpass` in
git's environment, dispatched in `main.rs` before clap, so no script or batch file is
involved. `GIT_EDITOR` and `GIT_SEQUENCE_EDITOR` keep `sh_quote` (Git for Windows runs them
through its sh) with forward-slash paths. Remote operations run with `CREATE_NO_WINDOW`,
`GIT_TERMINAL_PROMPT=0`, `SSH_ASKPASS=<exe>`, `SSH_ASKPASS_REQUIRE=force`, so an ssh prompt
fails fast instead of hanging on an invisible console (the Windows `setsid`). Surface
"dubious ownership" (`safe.directory`) errors verbatim.

**Agent hooks.** Claude's hooks are HTTP hooks (`agent.rs:175-205`); only the `SessionStart`
and `statusLine` helpers are commands. On Windows emit `"C:/…/workbench.exe" statusline`
(double quotes and forward slashes work in cmd and Git Bash).

**File watching.** `files/watch.rs:224` adds a watch per directory (up to 8000); on Windows
each open directory handle blocks renaming its parents. Use one recursive
`ReadDirectoryChangesW` watch on the root, filtered through `IgnoreChecker`; a buffer
overflow maps to `overflow: true`.

**Symlinks.** Creating one needs Developer Mode or admin: report `ERROR_PRIVILEGE_NOT_HELD`
clearly. Reading and containment are unaffected.

**Service: an HKCU `Run` value and a supervisor binary.**

- A second binary, `src/bin/workbenchw.rs` (`windows_subsystem = "windows"`; a stub
  elsewhere), starts `workbench.exe serve` with `CREATE_NO_WINDOW`, restarts it 5 s after a
  non-zero exit (parity with `RestartSec=5`), gives up after 5 failures within 60 s, and does
  not loop when a server started by hand holds the data dir.
- Its environment (`WORKBENCH_CONFIG_DIR`, `WORKBENCH_DATA_DIR`, `WORKBENCH_LOG`) lives in
  `%LOCALAPPDATA%\workbench\service.json`; PATH is not captured (a logon process already gets
  the user's PATH).
- A Start Menu `Workbench.lnk` runs `workbenchw.exe open`. `workbench service status` also
  reads `StartupApproved\Run` to report an entry disabled in Task Manager.
- Rejected: a logon scheduled task (`schtasks /SC ONLOGON` is refused for standard users in
  common setups, shows a console window, and its restart policy ignores the exit code); S4U
  tasks and Windows services (they lose Credential Manager and the desktop, and a service
  needs admin); a Startup-folder shortcut (same as the `Run` value but needs COM).

## 3. Tests

- About 914 tests in 170 files; most are logic-only and run as they are.
- A `cfg(test)` `testutil` module: `assert_private(path)` (replacing ~15 `mode() & 0o777 ==
  0o600` asserts), `symlink(target, link)` (skips without the privilege), `python()`
  (`python3`, `python` or `$WORKBENCH_TEST_PYTHON`), `script(dir, name, py)` (a shebang file
  with mode 0755 on Unix; `<name>.py` plus a `<name>.cmd` wrapper on Windows).
- Permanently `cfg(unix)`, with a Windows sibling where one exists: freedesktop trash,
  `mkfifo` (`apps/detect/tests.rs:673`), `renameat2` exchange races, systemd service tests
  (sibling: a scratch `HKCU\Software\Workbench-test-<rand>` key), `getifaddrs`, `/proc`
  parsers, `ptrace_scope`, the HUP-immune session test (sibling: killing a job that holds a
  Python grandchild).
- New Windows tests: DACL creation and detection, junction escape through `resolve_in_root`,
  device names and alternate data streams, drive-letter URIs, npm-shim parsing, `RmGetList`
  holders, CRLF line staging with `autocrlf=true`, the ConPTY DSR answer.
- `fake_ls.py` and `fake_dap.py` only need `python3` → `python()` (`lsp/tests.rs:58, 226`,
  `debug/tests.rs:15, 86`). The five bash fakes (`terminals/testdata/fake-*.sh`) become one
  `fake_cli.py`.
- CI runs `git config --global core.autocrlf false`; test repositories set it too. End-to-end
  timeouts scale by 2–3× on Windows.

## 4. Steps

Each step compiles and passes on Linux. S = under a day, M = 1–3 days, L = 3–6 days.

1. **S** `.gitattributes`; a `windows-latest` CI job running `cargo check --locked`, not
   required yet.
2. **M** `nix`/`libc` behind `cfg(unix)`, the Windows dependencies, `util::os` with the Unix
   code moved verbatim (`perm`, `fs`, `shutdown_signal`, `which`); rewire the ~45 files.
3. **M** lsp, debug, git/remote, history and the codex/pty scans through `os::proc` and
   `os::session` (Unix bodies).
4. **M** The shell and program-lookup sites through `os::shell` and `os::exe`; centralise the
   path helpers and harden the `/`-only validators.
5. **M–L** Windows `perm`, `fs` and `path`: DACLs, `MoveFileExW`, `resolve_in_root`
   hardening, dunce, the data-dir split, the v6-only bind.
6. **L** Windows `proc` and `session`: Job Objects, sysinfo, Restart Manager, shutdown
   events. From here `cargo build` passes on Windows and the CI job becomes required.
7. **L** ConPTY: EOF on close, the hold-back channel, default shells, npm-shim unwrapping,
   `.cmd` argument rules.
8. **M** Networking and desktop: GetAdaptersAddresses, GetExtendedTcpTable, browser launch,
   Recycle Bin.
9. **M** Git: askpass through the environment, editor paths, CRLF-aware diff and line
   staging, SSH_ASKPASS.
10. **M** LSP and debug: URIs, PATHEXT, rustup proxies, `python`, process list, tracer check.
11. **M** `service_windows.rs`, `workbenchw`, the icon resource.
12. **S** Reporting unsupported features (section 5).
13. **L** Tests (section 3); Windows `cargo test` becomes required.
14. **S** The release job; ARCHITECTURE.md (the `util::os` contract, Windows configuration,
    security and service).

**CI.** A `server-windows` job on `windows-latest`: `git config --global core.autocrlf
false`, checkout, setup-python, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache`
(workspaces: server), `cargo build --locked`, `cargo test --locked`.

**Release.** A `windows` job next to the Linux one: `server/.cargo/config.toml` sets
`[target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-feature=+crt-static"]` (no VC++
runtime needed); a pwsh smoke test (start with scratch directories, `Invoke-WebRequest` until
the page has `<div id="root">`, stop); package
`workbench-<v>-x86_64-pc-windows-msvc.zip` with `workbench.exe`, `workbenchw.exe`,
`install.ps1`, `conpty.dll`, `OpenConsole.exe`, LICENSE, README, CHANGELOG and the notices,
plus a `.sha256`; `publish` needs both jobs.

**A zip with `install.ps1`, not an MSI.** It mirrors the Linux archive and `install.sh`,
installs per user into `%LOCALAPPDATA%\Programs\Workbench` without elevation, updates the
user PATH, can move a running exe aside before copying the new one, and needs no WiX
toolchain (cargo-wix targets WiX 3 and per-machine installs). A winget "portable zip"
manifest can come later; revisit MSI once the binaries are code-signed. Users run
`Unblock-File` (Mark of the Web) or `-ExecutionPolicy Bypass`.

## 5. Risks, and what the first version leaves out

**Risks.** ConPTY quirks (EOF only when the pseudoconsole closes; `ClosePseudoConsole` can
block on Windows 10 if output is not drained). A child can start grandchildren in the gap
before it joins its Job (portable-pty has no suspended start; fork it if that matters).
`.cmd` injection wherever the resolver is bypassed. CRLF handling in line staging.
`aws-lc-sys` on MSVC (CMake is on the runners; check the first run and add a setup-nasm step
if needed). Sharing violations on rename and delete. A Windows Firewall prompt when binding
`0.0.0.0`. SmartScreen and antivirus reactions to an unsigned exe that spawns PTYs. The
Credential Manager target names keyring uses need documenting. Tests run 2–3× slower.

**Left out of the first version:**

- Dev containers: the bridge listener cannot bind the Docker Desktop VM's gateway
  (`devcontainer/mod.rs:557-565`), and the uid mapping (`ops.rs:156`) has no equivalent.
- Desktop notifications (they need an AppUserModelID shortcut).
- gdb attach hints and rust-gdb pretty printers; attach only through adapters that support it
  on Windows (debugpy, codelldb, lldb-dap).
- WSL and UNC project roots.
- The Docker Services tool window is marked experimental (untested).

**Reporting them.** `ApiError::unsupported(feature, reason)` answers HTTP 501 with `code:
"unsupported_platform"` and `feature`, shown like `not_configured` as a setup-help panel.
`GET /api/health` gains `os` and `unsupported: {feature: reason}`, so the UI hides or greys
out the Dev Containers tool window and "Attach to process…". MCP tools return the same text.
