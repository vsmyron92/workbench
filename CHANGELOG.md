# Changelog

## 0.3.0 - 2026-09-29

- **Windows (experimental):** the first release with a Windows build, for Windows 10 (1809
  or newer) and 11 on x86_64. Its whole test suite passes on GitHub's `windows-latest`
  (Windows Server 2025), a required CI job, and the release job installs the build with
  `install.ps1`, starts the server and installs again over it before publishing the
  archive. It has not been tried on a Windows 10 or 11 desktop yet
  ([status](docs/windows-port.md)). The archive holds `workbench.exe` (no Visual C++
  runtime needed), `workbenchw.exe`, `install.ps1`, and `conpty.dll` and `OpenConsole.exe`
  from Microsoft's ConPTY package (MIT, see the third-party notices).
- **On Windows:** terminals, agent sessions included, run in ConPTY. Shells are
  PowerShell 7 (`pwsh`), else Windows PowerShell. Agent CLIs start directly, and
  npm-installed ones start as `node` and their script, never through cmd.exe. Run
  configurations and detected commands also run in PowerShell, so a command written for
  bash needs PowerShell's syntax (Windows PowerShell 5.1 has no `&&`: install
  PowerShell 7). Language servers and debug adapters are found in their Windows forms
  (npm-installed ones run with Node). Rust built with MSVC debugs with lldb-dap or
  CodeLLDB, which you name in `[debug.default_adapter]`, since gdb reads only MinGW builds.
  `workbench service install` adds a Start Menu shortcut, and with `--enable` a sign-in
  entry, without administrator rights. The configuration is in `%APPDATA%\workbench` and
  the state in `%LOCALAPPDATA%\workbench`, both readable only by you and SYSTEM. A
  `keyring` secret reference reads Windows Credential Manager. Nothing Workbench reads in a
  project by itself follows a link to a network path or a device (`\\host\share\x`), which
  would make Windows sign in to that computer. Programs it starts (git, language servers,
  agents, your terminals) are not covered. What else works differently there, such as a
  terminal ending a browser or editor it started, is in
  [Install on Windows](docs/getting-started.md#install-on-windows-experimental).
- **Left out on Windows:** dev containers, the server's desktop notifications (turn on
  browser notifications instead), gdb attaching to a running process (native programs
  attach with lldb-dap or CodeLLDB, Python with debugpy), rust-gdb's pretty printers, and
  projects on a network share or inside WSL. The Services window (Docker Desktop) is
  marked experimental. Workbench hides these or says "Not available on Windows" and why
  (the API answers `unsupported_platform`, and `GET /api/health` lists them).
- **Git on Windows:** version control needs Git for Windows. A CRLF checkout
  (`core.autocrlf`, Git for Windows' default, or `eol=crlf` attributes) diffs as git reads
  it, with LF. A conflict resolved with edited text is written back with CRLF, and Local
  History keeps the last commit with the checkout's line ends. A repository that git
  refuses because of its owner (`safe.directory`) reports git's own message
  (`unsafe_repository`), which names the command that trusts it. Git Credential Manager
  never opens a sign-in window for Workbench's remote operations. It answers with what it
  has stored, and an https host it has nothing for fails at once.
- **Terminals:** `[terminals] shell` in `config.toml` sets the program and arguments of new
  shells (default: `$SHELL -l`, as before; PowerShell on Windows). Kill, Restart and Close
  now wait (up to 5 seconds) until the process's exit is recorded and saved. Before, they
  waited only until the process ended, or not at all for a process that had just ended by
  itself. Once they return, the terminal reads as exited. A terminal removed from history
  stays removed: a save still under way no longer writes its files back, so the terminal
  does not return at the next start.
- **Git credentials (security fix):** Workbench no longer answers a credential prompt
  whose user name contains `/`. Git before its January 2025 security releases prints that
  name unescaped, so a crafted remote or submodule URL could get the GitLab token sent to
  another host. Workbench's fetch, update, push and remote-branch deletion also no longer
  ask git's credential helpers for the GitLab host Workbench has a token for (the
  project's own `[repo.gitlab]` token, else `[gitlab]`), nor hand them that token to
  store. The token no longer ends up in Git Credential Manager, `~/.git-credentials`, a
  credential cache or a keychain. While Workbench has a token for that host, a stored
  credential no longer answers in its place, so a project's own token or a rotated one
  takes effect at once. Other hosts keep your helpers. If a credential helper of yours
  knew the GitLab host, Workbench's remote operations now use Workbench's token there, and
  that token needs Git-over-HTTPS access (write access to push). A token that earlier
  versions left with a credential helper stays there: remove it (for `store`, the GitLab
  host's line in `~/.git-credentials`).
- **Run configurations (security fix):** Unity detection takes the editor version from
  `ProjectVersion.txt` only when it consists of version characters (letters, digits, `.`,
  `_`, `-`). Otherwise a crafted `ProjectVersion.txt` could put shell syntax into the
  detected Unity runs.
- **Setup messages:** several messages now name the project's machine overlay,
  `config.toml` and the config folder this Workbench actually reads (`WORKBENCH_CONFIG_DIR`,
  `XDG_CONFIG_HOME`) instead of always `~/.config/workbench`. They are the messages about a
  missing toolchain, an unknown placeholder or an undefined ssh host, the Confluence setup
  hint and its "not available" message, and the TLS certificate and key placeholders in
  Settings › Remote. A default Linux install shows the same text as before.
- **API:** `GET /api/health` also reports `os`, what that OS leaves out (`unsupported`) and
  what it has only as experimental (`experimental`). Both are empty on Linux. An
  `unsupported_platform` error (HTTP 501) names its `feature`. `GET /api/atlassian/status`
  also returns `configFile`, the `config.toml` this server reads.
- **Build from source:** the build also makes `workbenchw`, the Windows launcher of
  `workbench service`. On Linux it is a stub that only prints a message, so install
  `workbench` alone, as before. The Linux archive still holds `workbench` alone.
- **Docs:** Help and the guides cover Windows (install, folders, the service, agents, what
  is left out). customization.md and ARCHITECTURE.md now give the `dotenv` secret
  reference in the form Workbench reads: `dotenv = { path = "…", key = "…" }`.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.3.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`.

Windows 10 (1809 or newer) or 11 on x86_64, experimental: in PowerShell, run
`Unblock-File` on `workbench-0.3.0-x86_64-pc-windows-msvc.zip`. It removes the Mark of the
Web that Windows puts on downloads, so the unpacked files do not carry it. Unpack the zip
with `Expand-Archive .\workbench-0.3.0-x86_64-pc-windows-msvc.zip -DestinationPath .`, then
run its installer:
`powershell -ExecutionPolicy Bypass -File .\workbench-0.3.0-x86_64-pc-windows-msvc\install.ps1`
(`-ExecutionPolicy Bypass` allows the unsigned script for this one run). It installs into
`%LOCALAPPDATA%\Programs\Workbench` without administrator rights and adds that folder to
your PATH. Then run `workbench serve --open` in a new terminal. The binaries are not
code-signed: SmartScreen or an antivirus may warn, and Windows 11's Smart App Control, when
on, blocks them.

Each archive has a `.sha256` to check it against (`sha256sum -c`, or `Get-FileHash` on
Windows).

**Update:** install the new release (or pull and rebuild), then restart Workbench
(`systemctl --user restart workbench.service` or the running `workbench serve`). Your
configuration and `~/.local/share/workbench` stay as they are. On Windows, a later
archive's `install.ps1` installs over this one, also while Workbench runs. Then restart
Workbench:
- If you started it with `workbench serve`, stop it (Ctrl+C) and start it again in a new
  terminal.
- If it runs as the service (from the Start Menu or at sign-in), run
  `workbench service stop` and open Workbench from the Start Menu.

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
