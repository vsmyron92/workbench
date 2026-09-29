# Changelog

## Unreleased

- **Windows (experimental):** the server is being ported to Windows 10 (1809 or newer) and
  11 on x86_64 ([plan and status](docs/windows-port.md)). Nothing of it has been tested on
  a real Windows machine yet. Operating-system code now goes through one layer
  (`util::os`); on Linux only the items marked "every OS" below change anything. On
  Windows, private files get an access list for you and SYSTEM only, child processes run in
  Job Objects, programs are found through `PATHEXT` (npm's `.cmd` shims start through
  `node.exe`), run commands go through PowerShell, a DLL loaded by name comes only from
  Workbench's own folder or System32, the configuration is in `%APPDATA%\workbench` and the
  state in `%LOCALAPPDATA%\workbench`. Terminals run in a pseudoconsole (ConPTY),
  PowerShell by default, and closing a terminal ends what it started, a browser or editor it
  opened that was not running yet included; `workbench service` installs a sign-in entry
  and a Start Menu shortcut that start `workbenchw.exe`, which supervises the server without
  a console window; git and ssh ask Workbench itself for credentials; each project has one
  recursive file watch, so its folders stay renamable; language servers, debuggers and
  detected run commands take their Windows forms; secret files written by Windows
  PowerShell 5.1 (UTF-16, or UTF-8 with a byte order mark) read as text. A link in a
  repository to a network path or a device (`\\host\share\x`) is never followed, so
  nothing Workbench reads by itself makes Windows sign in to another computer. A program
  installed while Workbench runs is found once it restarts, as the "not found" messages say.
  `GET /api/health` reports the OS and what it leaves out (dev containers, desktop
  notifications, gdb attach and rust-gdb's pretty printers, projects on network or WSL
  paths), and those features answer `unsupported_platform` with the reason.
- **Terminals (every OS):** `[terminals] shell` in `config.toml` sets the program and
  arguments of new shells (default: `$SHELL -l`; PowerShell on Windows). Kill, Restart and
  Close wait (up to 5 seconds) until the process's exit is recorded and saved, where they
  waited only until it ended (or, for a process that had just ended by itself, not at all),
  so once they return the terminal reads as exited. A terminal removed from history stays
  removed: a save still under way cannot write its files back, so it no longer returns at
  the next start.
- **Git credentials (every OS):** Workbench's fetch, update and push no longer ask git's
  credential helpers for the GitLab host Workbench has a token for (the project's own
  `[repo.gitlab]` token, else `[gitlab]`), nor hand them that token to store: it no longer
  ends up in Git Credential Manager, `~/.git-credentials`, a credential cache or a keychain,
  and a stored credential no longer answers in its place, so a project's own token, a
  rotated token or a removed one takes effect at once. Other hosts keep your helpers. On
  Linux this changes behaviour if a credential helper of yours knew that host: Workbench's
  remote operations now use Workbench's token there. Workbench also no longer answers a
  credential prompt whose user name contains `/` (git before its January 2025 security
  releases prints it unescaped), which a crafted remote or submodule URL could use to get
  the GitLab token sent to another host.
- **Git on Windows:** a working tree git checks out with CRLF over an LF index
  (`core.autocrlf`, Git for Windows' default, or `eol=crlf` attributes) shows in diffs and
  conflicts as git reads it, with LF; a conflict resolved with edited text is written back
  with CRLF; Local History keeps the last commit with the checkout's line ends. A
  repository git refuses for its owner (`safe.directory`) reports git's own message
  (`unsafe_repository`) instead of "not a repository". On Linux all of this stays as it was.
- **Run configurations:** Unity detection takes the editor version from
  `ProjectVersion.txt` only when it consists of version characters (letters, digits, `.`,
  `_`, `-`), since it becomes part of the detected commands (every OS).
- **Setup help:** the messages about a missing toolchain, an unknown placeholder or an
  undefined ssh host, the Confluence setup hint and its "not available" message, and the
  TLS certificate and key placeholders in Settings › Remote name the project's machine
  overlay, `config.toml` and the config folder where this Workbench reads them
  (`WORKBENCH_CONFIG_DIR`, `XDG_CONFIG_HOME`, `%APPDATA%` on Windows) instead of always
  `~/.config/workbench` (every OS; the same text on a default Linux install).
- **Releases:** the release workflow can also build
  `workbench-X.Y.Z-x86_64-pc-windows-msvc.zip` with `workbench.exe` (no Visual C++ runtime
  needed), `workbenchw.exe`, `conpty.dll` and `OpenConsole.exe` from Microsoft's ConPTY
  package (MIT, see the third-party notices) and `install.ps1`, which installs per user into
  `%LOCALAPPDATA%\Programs\Workbench`, adds it to PATH and can install over a running
  Workbench. The job installs the archive and starts the server before publishing it. A tag
  publishes it only once the repository variable `RELEASE_WINDOWS` is `true`; until then
  releases stay Linux-only. A build from source also makes `workbenchw`, which on Linux is
  a stub that only prints a message and is not installed; the Linux archive still holds
  `workbench` alone (every OS).

## 0.2.0 - 2026-09-29

- **Help:** the user documentation in the app, bundled so it works offline and on a phone:
  getting started, projects, agents, version control and CI, remote access and the phone
  (including Tailscale), running as a service, and `config.toml`. Open it with F1, the
  palette, the status bar's Help, or More → Help on a phone; search covers every page.
- **Docs:** a plan for porting the server to Windows ([windows-port.md](docs/windows-port.md)).

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.2.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`, then restart
Workbench (`systemctl --user restart workbench.service` or the running `workbench serve`).

## 0.1.0 - 2026-09-29

First public release.

- **Agents:** Claude Code, Codex, Kimi Code, Gemini CLI, Aider and custom CLIs in real
  terminals; permission requests answered from cards, toasts and the phone; Review Changes
  with per-file and whole-session revert; Claude Code Remote Control; MCP tools for the
  UI, CI, Confluence and Jira, Workspace cards, runs, code intelligence, the debugger and
  Local History.
- **Editor:** Monaco on CLion's keymap, language servers with semantic highlighting,
  hierarchies and structure (presets from rust-analyzer and clangd to Verible for
  Verilog and SystemVerilog and vhdl_ls for VHDL), a DAP debugger, Local History,
  bookmarks, compare, scratch files and an HTTP client for `.http` files.
- **Version control:** line staging and partial commits, changelists, the shelf,
  interactive rebase, bisect and a log graph.
- **Forges:** GitLab merge requests, pipelines, jobs and test reports; GitHub pull
  requests, Actions runs and artifacts; issues on both.
- **Atlassian:** Confluence reading and authoring with inline comments, attachments and
  labels; Jira issues and boards.
- **Running things:** detected run configurations, environments with health checks,
  previews and gated deploys, dev containers, a Services window for Docker and a Database
  window for PostgreSQL.
- **Workspace:** deliverable cards with sandboxed reports, galleries, PDFs and 3D
  comparisons, compatible with Mr. Mak Workspace's registry; four example cards in Home
  on the first start.
- **Phone:** an installable app with its own tabs and push notifications.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.1.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`, or build from
source. Windows is not supported yet.

**Update:** install the new release (or pull and rebuild), then restart Workbench
(`systemctl --user restart workbench.service` or the running `workbench serve`). Your
configuration and `~/.local/share/workbench` stay as they are.
