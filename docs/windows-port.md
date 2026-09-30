# Porting the server to Windows

**Status: experimental.** The server builds with MSVC and its whole test suite passes on
GitHub's `windows-latest` (Windows Server 2025), which is now a required CI job. It has not
run on a Windows 10 or 11 desktop yet.

- **Phase A (merged):** `.gitattributes`, the CI job, and the `util::os` areas `perm`, `fs`,
  `proc`, `shell`, `exe`, `path`, `net` and `desktop` with their Windows bodies (their shared
  Win32 helpers live in `util/os/win32.rs`). Call sites outside `util::os` are free of
  `cfg(unix)` / `cfg(windows)`; ARCHITECTURE.md has the contract ("Operating-system layer").
- **Phase B (merged):** terminals on ConPTY and Job Objects (`os::session`, §1.F); the service
  (`workbenchw.exe`, the `Run` value and the Start Menu shortcut in `os::autostart`, §2);
  git (askpass through the environment with `os::helper`, CRLF-aware diffs and line staging);
  file watching (one recursive watch, `os::watch`, §2); LSP, the debugger and detected
  commands in Windows forms; reporting unsupported features (`os::support`, §5); the Windows
  release job with `install.ps1`, and the user documentation (§4, step 14). The server loads
  DLLs by name only from its own folder and System32 (`os::dll`). The server and its tests
  compile for Windows.
- **First `windows-latest` run:** the MSVC build succeeded; `cargo test` passed 971 tests
  and failed 57. Fixed since: TOML fixtures that put a Windows path in a basic string (the
  GitHub and GitLab overlays, the hostile `.workbench.toml`), dev container paths shown or
  split with `\`, Local History's repair of a torn index line (`perm::set_len` on an append
  handle), `Event::set` of a name nothing holds (ERROR_INVALID_HANDLE), a terminal exit
  announced before it was recorded (every OS) and a reused leader pid (`session::Handle`),
  and tests that assumed `sh`, `/etc` or `/`-joined paths.
- **Second `windows-latest` run:** 1033 passed, 0 failed, 3 ignored (the same three as on
  Linux), and `install.ps1` installed the build. The job is required from here on.
- **Merge-readiness review:** fixed since (the paragraphs named are in §2): Workbench's
  GitLab token kept out of git's credential helpers, Credential Manager included, and
  askpass's reading of prompts ("Git", every OS); repository links to network paths never
  followed ("Links to other computers"); PowerShell's CLIXML errors on a pipe;
  `NoDefaultCurrentDirectoryInExePath` for the programs terminals start ("Program lookup");
  a restart during an exit's save that left the new process reading as exited (every OS);
  setup messages that named `~/.config/workbench`; `workbench service` from another Windows
  session ("Service"); and Local History after a watcher overflow ("File watching").
- **Packaging, after 0.3.0:** `install.ps1 -Uninstall` (§4, "A zip with `install.ps1`"),
  which CI runs after the install; and an application manifest in both executables (§1.A):
  Windows 10 and 11 as supported systems, Common Controls 6 for the message boxes of
  `workbenchw.exe` and the supervisor, `longPathAware`, and `asInvoker`. A `cfg(windows)`
  test checks in its own process that Windows applies it (GetVersionExW reports 10, comctl32
  loads in version 6). How the message boxes look is left for a desktop check.
- **Next:** real Windows 10 and 11 desktops (§5): ConPTY terminals with agent CLIs, the
  service and its Start Menu shortcut (and the look of its message boxes), git over SSH and
  HTTPS, language servers. Until then a tag publishes the Linux archive alone: the release
  workflow builds the Windows archive on a tag only once the repository variable
  `RELEASE_WINDOWS` is `true` (by hand it always does).

This is the plan for a native `x86_64-pc-windows-msvc` build that works on Windows 10 and
11, with Linux behaviour unchanged. File and line references are from 0.1.0 (commit
`493a66e`) and will drift.

Estimated size: 6–8 engineer-weeks, in 14 steps that each compile and pass on Linux.

## Core idea

Every OS-specific site goes through one new core module, `server/src/util/os/`
(`mod.rs` and a file per area, holding its `cfg(unix)` and `cfg(windows)` bodies), with the
areas `perm`, `fs`, `proc`, `session`, `shell`, `exe`, `path`, `net` and `desktop`. The Unix
bodies are today's code, moved verbatim from the call sites, so Linux behaviour stays
identical by construction. Windows-only behaviour is always `cfg(windows)`.

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
  - `dunce` (already in the lock). The Start Menu shortcut is written through the shell's
    ShellLink COM object with windows-sys (`os::autostart`), not `mslnk` (unmaintained since
    2022, bitflags 1, a subset of the format).
- Build-dependency `winresource` (icon, version resource and the application manifest
  `packaging/windows/workbench.manifest`, a no-op elsewhere). The manifest declares Windows
  10 and 11 (`supportedOS`: without it Windows treats the programs as written for Windows
  8), Common Controls 6 (message boxes in the current style), `longPathAware` (no MAX_PATH
  limit where `LongPathsEnabled` is set; the Recycle Bin's `SHFileOperationW` keeps it, and
  `os::fs` refuses longer paths there) and `asInvoker`. It is linked into the test
  executables too. Not done:
  the Windows dev-dependency `junction`, since the tests make junctions with `cmd /c mklink /J`.
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
- Done (`util/os/session.rs`): the session registry holds a `ProcGroup` and the closure that
  closes the pseudoconsole (the session owns it until it is over, so a `Pty` dropped early
  does not hang up its background jobs). It stays registered while a `session::Handle` of it
  lives (the `Pty`, the lingering-process watch), so a reused leader pid never makes a
  terminal follow or kill another terminal's session. The pump thread always drains the
  pipe, also after the reader stopped, so `ClosePseudoConsole` never waits for good. A secret
  is also masked when ConPTY's repainting puts escape sequences between its characters
  (`session::REPAINTS`). A GUI program started from a terminal joins its job like any other
  process, counts as lingering once the terminal's own process has exited, and ends with the
  terminal: Kill, Close and Restart close the pseudoconsole (which a GUI program does not
  notice), then `TerminateJobObject` ends it; Workbench stopping ends its terminals' sessions
  the same way (and the job is `KILL_ON_JOB_CLOSE` besides). A process that asks to leave the
  job (`CREATE_BREAKAWAY_FROM_JOB`) may (`JOB_OBJECT_LIMIT_BREAKAWAY_OK`), as a daemon leaves
  a Unix session: the service `workbench service install --enable` starts from a terminal.
- **A known Windows difference:** a browser or an editor that a terminal's program starts
  when it was not running yet (the sign-in page an agent CLI opens, `start <url>`, `code .`)
  is in that job too unless it leaves it, and closing, restarting or killing the terminal
  then ends it, every window of it. One that already runs only receives the page or folder
  and is not affected. On Linux such a program is in the terminal's session and ends
  likewise, unless its launcher starts it in a session of its own (`setsid`; Node's
  `detached: true`, which VS Code's `code` and the `open` package use there), so there it
  usually outlives the terminal; on Windows `detached` only means no console, and the
  program stays in the job. Not changed: a job cannot let a process go, and sparing GUI
  programs at Kill would leave running a GUI app under development that a run
  configuration started. The user documentation says to start the browser or editor
  outside Workbench first (getting-started, Help).

**G. `/proc` introspection**

- Sites: `pty.rs:902-964` (`cli_running_in`: cwd and cmdline of other processes);
  `codex.rs:366-371, 453-470` (`/proc/pid/fd`); `debug/procs.rs:38, 80, 87-147` and
  `debug/routes.rs:113` (attach list, `ptrace_scope`); `debug/session.rs:913-924`
  (`TracerPid`); `agent.rs:245-260`, `git/mod.rs:113`, `askpass.rs:117` (the "` (deleted)`"
  executable path); `service.rs:282`; `app.rs:167` (`bindv6only`).
- Windows: sysinfo for the process list, cwd, cmdline, parent and start time; the Restart
  Manager (`RmGetList`) for "who holds this rollout file open"; `CheckRemoteDebuggerPresent`
  for `TracerPid`; `ptrace_scope` is `None`. No " (deleted)" fallback: a running exe cannot be
  replaced on Windows. The attach picker guesses a process's language from its image name
  (`node.exe`, `javaw.exe`), else from the program its command line starts with, `\` paths
  and unquoted `C:\Program Files\…` included (`debug/procs.rs`).

**H. Shells → `os::shell`**

- Host-side sites: `terminals/mod.rs:1236-1241`; `apps/runs.rs:798, 1218, 1248, 1348`
  (`bash -lc`); `apps/remote.rs:102`; `debug/session.rs:999`; `platform/notify.rs:222-229`
  (`sh -c`); `devcontainer/ops.rs:193`. POSIX quoting: `apps/expand.rs:185 shell_quote` (47
  callers building `bash -lc` commands), `terminals/input.rs:69-75 quote_path`,
  `agent.rs:262-268, 821` (the statusline command). `/bin/sh` inside containers stays.
- API: `interactive()`, `run_argv(cmd)`, `quote(s)`, `posix_quote(s)`, `helper_command(exe, args)`.
  `quote` is for the local run shell only; anything bound for a POSIX shell elsewhere (an ssh
  host through `apps::remote::quote`, a detected ssh deploy, the dev container scripts) takes
  `posix_quote`, the same on every OS.

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
- `apps/runs.rs:480-497` (`fuser -k`) → `GetExtendedTcpTable` for the owning pid and bind
  time. Windows names only the process that bound the socket, so the owner and the processes
  it started after binding (which can hold an inherited copy, as a reloader's worker does) are
  terminated, parents first. Only the same user's processes are terminated, and a pid that
  another process has taken since the bind is left alone.

**L. Desktop integration**

- `util/mod.rs:27-47 open_in_browser`: Edge or Chrome from the registry's App Paths with
  `--app=`, else `ShellExecuteW`; never `cmd /c start` (cmd interprets `&` in a URL).
- Trash (`files/trash.rs`, `git/ops.rs:182-185`) → `SHFileOperationW(FO_DELETE,
  FOF_ALLOWUNDO | …)`.
  - What the bin will not take is refused first: no bin on the drive, the bin turned off, a
    file larger than the bin.
  - An item Windows still cannot recycle (a folder larger than the bin) gets Windows' question
    on the desktop, and the request stops waiting after 60 s. Later: `IFileOperation` with a
    progress sink that refuses the item instead.
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
Folders other programs record compare the same way (`os::path::same_dir`, `below_dir`):
Claude Code's `~/.claude.json` project keys, Gemini's `projects.json` (lowercased there, as
Gemini writes it), and a hook's `cwd` that shortens a permission prompt's path. Local
History keys a file an agent names by its case on disk (`os::path::on_disk_case`), so a
hook spelling it in another case adds to its one history. Linux compares as before.

**LSP URIs.** Emit `file:///C:/…`; accept `/c:/` and `/C%3A/`; match the project root with a
case-insensitive drive letter (servers often lowercase it); `lsp-src://pid/C:/…`.

**Paths in the browser.** The web app reads the server's OS from `GET /api/health`
(`features/files/paths.ts`). On a Windows server a drive path (`C:\…`, `C:/…`) is absolute
and `\` also separates names (tab titles, breadcrumbs, a debug frame's file, Markdown links
in a file outside the project); paths the server and a debug adapter wrote compare without
regard to `/` versus `\` or ASCII case (`samePath`). A file outside the project has the
model URI `file:///~abs/C%3A%5Cx` and parses back to `C:\x`; a debug stop in such a file
opens its source; Copy Path and drag and drop join a project path to the root with `\`.
Linux URIs and paths are unchanged.

**Line endings.** Add `.gitattributes` (`* text=auto eol=lf`, `*.ps1` and `*.cmd`
`eol=crlf`). In the git slice read `git ls-files --eol <path>`: when the index has LF and the
working tree CRLF, strip `\r` from the working-tree side for the diff and for the patch given
to `git apply --cached`, and put CRLF back when rolling lines back into the working tree.
Done (`git/eol.rs`, on Windows: `eol::FOLLOWS_GIT`): git's own diffs already read such a
file with LF, so the staging patches were right and `git apply` writes CRLF back by itself;
what was missing is the diff's `modified` side and a conflict's `merged` text (now LF, like
the hunks) and a conflict resolved with edited text (written back with CRLF). "Converted"
follows git: `ls-files --eol` (`i/lf`, `w/crlf`, the `attr/` column) and `core.autocrlf` when
no attribute decides; anything else keeps its bytes. Local History keeps "Last commit (HEAD)"
of a file with CRLFs on disk with the line ends a checkout writes (`files/history`). Linux
keeps every file byte for byte, with no extra git call: following git's conversions there
too (they matter with `core.autocrlf` or `eol=crlf` attributes) would be a Linux change for
the owner to decide.

**Program lookup, `.cmd` shims and BatBadBut.** portable-pty resolves PATHEXT and launches
`claude.cmd` with MSVCRT quoting (`cmdbuilder.rs:581-606, 702`): command injection through
cmd.exe for an agent prompt passed in argv. So Workbench always hands portable-pty an
absolute path; npm shims (codex, gemini, typescript-language-server, pyright…) are unwrapped
to `node.exe <package script>` (parsed from the `.ps1` next to the shim), which also avoids
cmd's "Terminate batch job (Y/N)?" and its current-directory search for `node`. Real
`.bat`/`.cmd` files run only when their arguments contain none of `%!^&|<>"` or newlines;
otherwise the prompt is pasted instead. Prefer a native `claude.exe` in
`%USERPROFILE%\.local\bin`. Set `NoDefaultCurrentDirectoryInExePath=1` for non-interactive
shells Workbench starts. Done (`os::exe::child_env`): the programs and command lines
Workbench starts get it (`run_cmd`, `exe::command`, `exe::configured`, `shell::command`,
language servers), and so does every terminal but an interactive shell's: runs, pre-launch
steps, debuggees, env and one-off commands, agent CLIs. An interactive shell keeps Windows'
usual lookup, since its user types the commands: in cmd.exe `build` runs the `build.bat` in
the current folder, as in any other terminal. PowerShell and bash never take a program from
the current folder by a bare name, and a shell that is a batch file (`[terminals] shell`)
gets the variable as every batch file does.

**Shells.** Terminals: `pwsh.exe -NoLogo`, then `powershell.exe -NoLogo`, configurable in
`[terminals] shell`. Runs, pre-launch steps and the notify command: `pwsh -NoLogo -NoProfile
-EncodedCommand <base64 UTF-16LE>`, which survives portable-pty's quoting. `run_shell = "cmd"
| "powershell" | "bash"` selects cmd, PowerShell 5.1 (no `&&`) or Git Bash, shown in the
run's argv; `quote()` follows the choice. (Not done: runs always use PowerShell, `pwsh` else
Windows PowerShell; there is no `run_shell`.) Add `WT_SESSION` and `WT_PROFILE_ID` to
`PARENT_TERMINAL_VARS`. (Done: `os::session::PARENT_TERMINAL_VARS`, which terminals clear
besides their own list.) `quote()` gives a word its value; a native program gets what
PowerShell makes of it. Only Windows PowerShell 5.1 puts a word with a space in double
quotes as it is, so a final `\` escapes the closing quote. Every pwsh passes it intact: it
doubles the trailing `\`s where it writes the command line itself (`Legacy`, every pwsh
before 7.3, and the `Windows` default for batch files) and quotes by the MSVCRT rules
elsewhere. A doubled `\` would suit 5.1 and break every pwsh, and the quoting knows neither
the PowerShell nor the program, so it is left as it is (`os::shell::ps_quote`, tested
against 5.1 and pwsh in both ways). Workbench's own words never end so, and detection does
not offer a repository name that does (`native_quoting_safe`): 5.1 runs commands wherever
pwsh is not installed.

**PowerShell's errors on a pipe.** Started with `-EncodedCommand`, not interactive and with
stderr redirected (a service's stop command, a local version or health probe: `run_cmd`),
PowerShell writes its own error, warning, verbose, debug, progress and information records
to stderr as CLIXML (`#< CLIXML` then `<Objs …><S S="Error">…_x000D__x000A_</S>…`),
assuming PowerShell reads it. `os::shell::readable_stderr` turns CLIXML back into what the
console would show (error lines as they are, `WARNING: `… prefixes, records that are objects
dropped, a native program's raw stderr kept) and drops the colour escapes pwsh 7's error
view puts in it. The run shell passes no `-OutputFormat`: pwsh 6.2 and later given
`-OutputFormat Text` write errors as text, but warning, verbose and debug lines, coloured,
to stdout, where a version probe reads the version (Windows PowerShell 5.1 has no such
exception). Stdout thus carries only the command's output in both PowerShells. Terminals
are unaffected: their stderr is the console.

**Detected commands.** Detection writes POSIX forms (`.venv/bin/python`, `python3`, `cmake
--build … && ./bin`, `cd dir && ./x.sh`), and the `health.via_host` probe is `curl -o
/dev/null` (in Windows PowerShell 5.1 `curl` is `Invoke-WebRequest`). Local runs need
Windows forms: the venv's `Scripts\python.exe`, `os::exe::python()`, `.\bin.exe`, no `&&`
under 5.1 (or pwsh 7 required); a local via_host probe runs `curl.exe -o NUL`;
`debug::derive::is_python` accepts `python.exe` and `py`, and in a Go module a launch
configuration's `.\cmd\api` is Go like `./cmd/api` (`debug::launch::language_of`). Deploys
and probes for an ssh host keep the POSIX forms.

**Process trees.** Job Objects replace process groups and the `/proc` session scan;
`TerminalInfo.lingering` is the job's process count minus one. `KILL_ON_JOB_CLOSE` matches
Linux, where closing the PTY hangs up its processes.

**Signals.** `\x03` typed in a terminal becomes CTRL_C_EVENT through ConPTY. Non-PTY children
get a hidden console of their own, so the server's Ctrl-C never reaches them (the
counterpart of `process_group(0)`). `ExitInfo.signal` is always `None`, and a process ended
from outside (Task Manager's End task, `taskkill /F`: `TerminateProcess`) only has the exit
code it was given, 1, like one that exited with 1 itself: its run ends Failed. Kill, Close and
Restart in Workbench are recorded (`ExitInfo.terminated`, `Pty::note_killed`, read by the
waiter as the process exits), so a run whose terminal Workbench closed ends Exited and
terminated, as on Linux, where a hang-up, terminate, kill or interrupt signal from anywhere
counts too (`os::session::wait`). Windows keeps
"ignore Ctrl-C" per process and hands it down: a process started with
`CREATE_NEW_PROCESS_GROUP` has it, so a server below one would start every terminal with
Ctrl-C dead. `serve` clears it first (`os::proc::enable_ctrl_c`, as Windows Terminal does),
and `os::autostart` starts the supervisor without a new process group.

**Terminals (ConPTY).** Resize is `ResizePseudoConsole`. ConPTY gives no EOF when the child
exits: close the pseudoconsole once the leader has exited and the job is empty, on a blocking
thread while the reader keeps draining. portable-pty creates the console with
`INHERIT_CURSOR`, so ConPTY sends `ESC[6n` and waits; the server answers that first query
from its mirror whether or not a client is attached, and keeps it from the clients
(`session::ASKS_CURSOR`). Ship a side-loaded `conpty.dll` and `OpenConsole.exe` (the
Microsoft.Windows.Console.ConPTY package, MIT), which portable-pty loads from the exe's
folder; the inbox ConPTY renders poorly on Windows 10. portable-pty loads it by bare name,
which would also search the current directory and `PATH`, so `serve` first limits the DLL
search to the exe's folder and System32 (`os::dll`, `SetDefaultDllDirectories`).

**Git.** `GIT_ASKPASS` is the absolute `workbench.exe` with `WORKBENCH_HELPER=askpass` in
git's environment, dispatched in `main.rs` before clap, so no script or batch file is
involved. `GIT_EDITOR` and `GIT_SEQUENCE_EDITOR` keep `sh_quote` (Git for Windows runs them
through its sh) with forward-slash paths. Remote operations start with no console at all
(`DETACHED_PROCESS` in `ProcGroup::prepare_session`, the Windows `setsid`: git then
starts ssh without one too, and ssh fails instead of prompting on a hidden console), plus
`GIT_TERMINAL_PROMPT=0`, `SSH_ASKPASS=<exe>` and `SSH_ASKPASS_REQUIRE=force`. Surface
"dubious ownership" (`safe.directory`) errors verbatim.

Done (`util::os::helper`, git slice). Unix keeps its `#!/bin/sh` wrapper: the variable route
is not the same there (every hook, ssh and credential helper git starts would inherit
`WORKBENCH_HELPER`, and the argv changes). The dispatch takes a call only with the variable
set and a single argument that is not a subcommand or an option, because the rebase's
`workbench git-editor …` and hooks run under the same environment. What Windows users should
know: Workbench's askpass answers only the configured GitLab host over https (the
project's `[repo.gitlab]` host when it has its own token), and for that host remote ops empty
git's credential helper list (`-c credential.https://<host>.helper=`, every OS), so
Credential Manager is neither asked for it (a sign-in stored there does not answer for
Workbench) nor handed Workbench's token to store. Other hosts go to git's credential helpers
(Git for Windows installs Credential Manager) and then to askpass, which refuses them.
Remote ops run with `GCM_INTERACTIVE=never`: Credential Manager returns what it has stored
but never opens its sign-in window, so an https host it knows nothing about fails at once.
An ssh key with a passphrase must be loaded in an agent the ssh git uses
can reach, and a new host must be accepted once in a terminal (`known_hosts`): remote
operations cannot prompt and fail instead, and their message says so (the OpenSSH
Authentication Agent service is the agent there; a changed or revoked host key is flagged,
not offered for acceptance). A repository an administrator created, or one
on a drive without owners (FAT, exFAT, some network shares), stops with git's
`safe.directory` message, which names the command that trusts it (`403 unsafe_repository`,
Windows only: `os::fs::FOREIGN_OWNERS`; on Linux such a folder still reads as "not a git
repository", a change there being the owner's to decide). The git tool windows show the
message with its line breaks and a button that copies that command, the status bar reads
"Untrusted repository", the project gets a warning naming the folder and the command, a
deploy answers `unsafe_repository` too, and the GitLab and GitHub pollers log it once per
project (`util::git::refuses`, shared with the git slice).

**Agent hooks.** Claude's hooks are HTTP hooks (`agent.rs:175-205`); only the `SessionStart`
and `statusLine` helpers are commands. On Windows emit `"C:/…/workbench.exe" statusline`
(double quotes and forward slashes work in cmd and Git Bash).

**File watching.** `files/watch.rs:224` adds a watch per directory (up to 8000); on Windows
each open directory handle blocks renaming its parents. Use one recursive
`ReadDirectoryChangesW` watch on the root, filtered through `IgnoreChecker`; a buffer
overflow maps to `overflow: true`. Done in `util::os::watch`: notify 8's Windows watcher
drops overflows silently (the rescan event is in notify 9, a release candidate) and
notify-debouncer-full's Windows file-id cache walks the whole tree, following links, on
every watch and created folder, so Windows gets its own watcher (a thread per watched
directory that makes every request, since Windows cancels a thread's pending I/O when it
exits; 64 KB buffer; 8.3 names in notifications made long again; `Flag::Rescan` on
overflow) under the same debouncer with no cache. The folders whose changes are kept come
from the Linux walk itself (`dirs`, gitignore-aware, ignore files above the root
included), so both report the same paths. A watch that stops on an error is made again
(after 1 s, doubling), with `overflow: true`. Git dirs outside the root (a subdirectory
project, a linked worktree) keep their own watches. Linux is unchanged: inotify's queue
overflow is still not reported (reporting it would be a Linux change for the owner to
decide). The one watch also receives `node_modules`, `target` and `.git` traffic, so a burst
there (`npm install`, a build) can overflow it: Local History then snapshots the paths the
batch did report and the files the walk finds modified since just before the previous batch
(`files::watch::changed_since`), unless those are more than 500 (a checkout, left to the VCS
as on Linux).

What Windows users notice: the folders that contain an open project cannot be renamed or
moved while Workbench runs (as with any IDE); a linked worktree's project also holds its
main checkout's `.git`. Folders inside the project can be renamed freely. Names that
differ only in case are one file: creating `A.txt` next to `a.txt` reports that it
exists, and renaming `a.txt` to `A.txt` changes only the case.

**Symlinks.** Creating one needs Developer Mode or admin: report `ERROR_PRIVILEGE_NOT_HELD`
clearly. Reading and containment are unaffected. In the files slice only a copy creates
links (a copied folder's symlinks): without the privilege the copy fails with that message
and leaves nothing half-copied. Junctions list as links and are not followed out of the
project.

**Links to other computers.** Opening a link whose target is `\\host\share\x` makes
Windows sign in to that host with the user's credentials (an NTLM response the host can
crack or relay), and a repository cloned with `core.symlinks` can hold one (Git for
Windows fixed the same attack on its own checkout). So Workbench never follows a link to
a network path or a device: `os::path::canonicalize` follows links one at a time, reading
each target first (`read_link`, which opens the link itself), and refuses UNC paths in
every spelling, device and NT paths (`\\.\`, `\\?\` other than a drive or a volume,
`\Device\…`) and rooted targets; `os::path::leaves_machine` answers the same question
for a path about to be opened. Everything Workbench reads in a project by itself goes
through them: containment (`resolve_in_root` refuses such a path), listings (a "broken"
link), ignore files (`IgnoreChecker`, and the walks: in the folders they visit and above
their start, above where its links lead too, since the `ignore` crate reads the ignore
files above the resolved start), the watcher's new folders, detection's files looked up
by name, `.workbench.toml`, the project's MCP files, run and debug configurations, `.git`
files naming a git dir, and the projects under a root.
Linux follows links as it always did.

**Service: an HKCU `Run` value and a supervisor binary.** (Done: `platform/service_windows.rs`,
`os::autostart`, `src/bin/workbenchw.rs`; see "Service install" in ARCHITECTURE.md.)

- A second binary, `src/bin/workbenchw.rs` (`windows_subsystem = "windows"`; a stub
  elsewhere), starts `workbench.exe serve` with `CREATE_NO_WINDOW`, restarts it 5 s after a
  non-zero exit (parity with `RestartSec=5`), gives up after 5 failures within 60 s, and does
  not loop when a server started by hand holds the data dir. As built, `workbenchw` only
  starts the hidden `workbench service run` (the supervisor) or `service open` without a
  console: it cannot use the server's modules (no library target), and the supervisor needs
  the data dir and the stop event's name. `workbench service stop` sets the server's stop
  event and the supervisor's (`<stop event>-service`); "a server holds the data dir" is its
  stop event existing, which, unlike runtime.json, cannot be stale. The events are `Local\`,
  one set per Windows session. `Global\` events would reach across sessions without any
  privilege (`SeCreateGlobalPrivilege` is checked only when a file mapping or symbolic link
  is created there), but every account can create names in `Global\`, and these are
  predictable (a hash of the data dir's path): another account could create a data dir's
  name first and leave its server without a stop event. A desktop session's own namespace is
  out of other accounts' reach (session 0's, where SSH sign-ins run, is the global one). A
  private namespace bounded by the user's SID cannot be squatted, but it closes with the
  process that created it (later `OpenPrivateNamespace` calls fail), and the server and its
  supervisor come and go apart. So a server in another session (the desktop one, seen from
  an SSH sign-in in session 0) is found by runtime.json's live pid in another session
  answering on its port; `status` names it, `stop` and `install --enable` refuse with that
  reason, `service open` uses it, and message boxes are skipped where no one could answer
  them (session 0).
- Its environment (`WORKBENCH_CONFIG_DIR`, `WORKBENCH_DATA_DIR`, `WORKBENCH_LOG`) lives in
  `%LOCALAPPDATA%\workbench\service.json`; PATH is not captured (a logon process already gets
  the user's PATH). `install --enable` and `service open` start the supervisor in the user's
  sign-in environment (`os::env::user_default`) plus these, as the `Run` entry does, not in
  the environment of the shell they run in.
- A Start Menu `Workbench.lnk` runs `workbenchw.exe open`. `workbench service status` also
  reads `StartupApproved\Run` to report an entry disabled in Task Manager.
- `install --enable` over a running service starts the new supervisor outside its own job
  (`CREATE_BREAKAWAY_FROM_JOB`), since stopping the old server closes the terminal it may run
  in. Workbench terminals' jobs allow that (`JOB_OBJECT_LIMIT_BREAKAWAY_OK`,
  `ProcGroup::attach_terminal`); from a terminal whose job does not, it restarts nothing and
  says so.
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
  `debug/tests.rs:15, 86`). The five bash fakes (`terminals/testdata/fake-*.sh`) became one
  `fake_cli.py`, used on every OS (on Windows through an npm-style shim, so the tests take
  the shim unwrapping path); the terminals' end-to-end tests run Python programs. The
  Services test's fake docker is `devcontainer/testdata/fake_docker.py` likewise (run as
  `docker` on Unix, through a `docker.cmd` and `docker.ps1` shim on Windows).
- A fixture that writes a path into TOML writes it as a TOML value (`toml::Value`), never
  spliced into a basic string, where a Windows path's `\` is an escape.
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
   events. From here `cargo build` passes on Windows (the CI job became required with
   step 13).
7. **L** ConPTY: EOF on close, the hold-back channel, default shells, npm-shim unwrapping,
   `.cmd` argument rules; detected commands in Windows forms (§2).
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
(workspaces: server; kept when tests fail), `cargo build --locked`, `cargo test --locked
--no-fail-fast`, then `install.ps1` under Windows PowerShell 5.1 whenever the build
succeeded: an install, then `-Uninstall`, refused while Workbench runs from the folder (the
refusal must name the server's pid) and then checked to remove the service's shortcut, the
folder and the PATH entry (the rest of the user PATH and its type unchanged). Informational (`continue-on-error`) until step 13;
required since (done: see the status at the top).

**Release.** A `windows` job next to the Linux one: `server/.cargo/config.toml` sets
`[target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-feature=+crt-static"]` (no VC++
runtime needed; `dumpbin /dependents` checks it); `conpty.dll`
(`runtimes/win-x64/native/`) and `OpenConsole.exe` (`build/native/runtimes/x64/`) from the
`Microsoft.Windows.Console.ConPTY` package on nuget.org, version and SHA-256s pinned in the
workflow (conpty.dll looks for `OpenConsole.exe` beside itself first); a pwsh smoke test
(`install.ps1` under Windows PowerShell 5.1 into a scratch prefix, start with scratch
directories on a free port, `Invoke-WebRequest` until the page has `<div id="root">`,
install again over the running server, stop); package
`workbench-<v>-x86_64-pc-windows-msvc.zip` with `workbench.exe`, `workbenchw.exe`,
`install.ps1`, `conpty.dll`, `OpenConsole.exe`, LICENSE, README, CHANGELOG, the
notices and Windows Terminal's `NOTICE.md` of that package's release (`CONPTY_NOTICE.md`),
plus a `.sha256`; `publish` needs both jobs. While the port is unvalidated, a tag runs the
Windows job only when the repository variable `RELEASE_WINDOWS` is `true`, and `publish`
otherwise ships Linux alone.

**A zip with `install.ps1`, not an MSI.** It mirrors the Linux archive and `install.sh`,
installs per user into `%LOCALAPPDATA%\Programs\Workbench` without elevation, updates the
user PATH, can move a running exe aside before copying the new one, and needs no WiX
toolchain (cargo-wix targets WiX 3 and per-machine installs). A winget "portable zip"
manifest can come later; revisit MSI once the binaries are code-signed. Users run
`Unblock-File` (Mark of the Web) or `-ExecutionPolicy Bypass`.

`install.ps1 -Uninstall` (same `-Prefix`) is the way back. It changes nothing while a
program runs from the folder (a process whose executable is there: the server, the
supervisor, a terminal's `OpenConsole.exe`, a `*.old` still running), and so never runs in a
terminal of the Workbench it removes. It lists the processes with
`System.Diagnostics.Process` and reads each one's path with `QueryFullProcessImageNameW`
(`PROCESS_QUERY_LIMITED_INFORMATION`, which an elevated process's integrity level does not
block), not WMI. It fails closed: when the processes cannot be listed, or a `workbench.exe`,
`workbenchw.exe` or `OpenConsole.exe` of the current session has a path it cannot read, it
changes nothing and says so (another account's processes in other sessions are not
counted). It runs `workbench service uninstall [--name N]` only for the services whose
`Run` value or Start Menu shortcut starts that folder's `workbenchw.exe`, so another
install's service stays. It deletes the files `install.ps1` puts there (the payload,
`*.old`, `*.new`), removes exactly the PATH entry `install.ps1` added, keeping the value's
type (`REG_EXPAND_SZ`), and broadcasts `WM_SETTINGCHANGE`, then deletes `workbench.exe`
and the folder when it is empty then. Until `workbench.exe` goes, a second run finishes an
interrupted one; a folder that exists without `workbench.exe` is not touched (and when it
is still on the PATH, it is refused, naming the manual step). The configuration and data
folders stay, and it prints where they are.

## 5. Risks, and what the first version leaves out

**Risks.** ConPTY quirks (EOF only when the pseudoconsole closes; `ClosePseudoConsole` can
block on Windows 10 if output is not drained). A child can start grandchildren in the gap
before it joins its Job (portable-pty has no suspended start; fork it if that matters).
`.cmd` injection wherever the resolver is bypassed. CRLF handling in line staging.
Installers change `PATH` in the registry only, so Workbench's own `PATH` stays the one it
started with. Done (`os::env`): `CreateEnvironmentBlock` for the process's token, without its
own variables, gives the environment a new sign-in gets, re-read once HKLM's or HKCU's
`Environment` key changes (`RegNotifyChangeKeyValue`). New terminals, runs and agents get its
`Path`, then Workbench's own absolute entries it lacks (a virtual environment it was started
from); before, portable-pty put the registry's `Path` over Workbench's, dropping those. A
lookup (`os::exe::which`) that misses tries its folders, and what Workbench starts by itself
outside a terminal (language servers, debug adapters, secret and service commands) gets them
after its own `PATH` (`os::exe::program_env`), so a program installed while Workbench runs
(Node.js, Python, rustup, an agent CLI, a language server) is found without a restart and
finds what it runs in turn (gopls its `go`). What the server starts by name through std (git,
and rustc for gdb's pretty printers) keeps its own `PATH` until it restarts; git's "not
found" message says so (`os::exe::INSTALLED_SINCE`). The other variables an installer sets
(`JAVA_HOME`) reach new terminals only (portable-pty reads the `Environment` keys).
`aws-lc-sys` on MSVC: 0.45 builds with its `cc` builder (no CMake) and, without NASM, links
the prebuilt NASM objects that rustls's `aws_lc_rs` feature enables (`prebuilt-nasm`), so
no setup-nasm step should be needed; check the first run. Sharing violations on rename and
delete. A Windows Firewall prompt when binding
`0.0.0.0`. SmartScreen and antivirus reactions to an unsigned exe that spawns PTYs. Tests
run 2–3× slower. (A `keyring` reference `service/account` is the generic credential
`account.service`, documented in getting-started.)

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
Done (`util::os::support`): the keys are `devcontainer`, `desktopNotifications`, `gdbAttach`,
`rustGdbPrettyPrinters`, `networkRoots`, plus `experimental: {services}`. The dev container
chip, status item and commands are hidden; "Attach to Process…" stays (attach works through
lldb-dap, CodeLLDB and debugpy, which an attach by language picks over gdb) and its picker
shows the gdb note; the local browser notifies in place of the desktop.
