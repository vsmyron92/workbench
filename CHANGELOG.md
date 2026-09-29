# Changelog

## Unreleased

- **Windows (experimental):** the server is being ported to Windows 10 (1809 or newer) and
  11 on x86_64 ([plan and status](docs/windows-port.md)). Nothing of it has been tested on
  a real Windows machine yet. Operating-system code now goes through one layer
  (`util::os`), and Linux behaviour is unchanged. On Windows, private files get an access
  list for you and SYSTEM only, child processes run in Job Objects, programs are found
  through `PATHEXT` (npm's `.cmd` shims start through `node.exe`), run commands go through
  PowerShell, a DLL loaded by name comes only from Workbench's own folder or System32, the
  configuration is in `%APPDATA%\workbench` and the state in `%LOCALAPPDATA%\workbench`.
- **Releases:** the release workflow can also build
  `workbench-X.Y.Z-x86_64-pc-windows-msvc.zip` with `workbench.exe` (no Visual C++ runtime
  needed), `conpty.dll` and `OpenConsole.exe` from Microsoft's ConPTY package (MIT, see the
  third-party notices) and `install.ps1`, which installs per user into
  `%LOCALAPPDATA%\Programs\Workbench`, adds it to PATH and can install over a running
  Workbench. The job installs the archive and starts the server before publishing it. A tag
  publishes it only once the repository variable `RELEASE_WINDOWS` is `true`; until then
  releases stay Linux-only.

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
