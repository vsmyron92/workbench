# Getting started

Workbench is one program that runs on your machine and serves its UI to your browser.
Your projects stay ordinary folders, your agents stay ordinary CLI sessions, and your
tokens stay in the files and keyrings where you already keep them. You can stop using
Workbench at any time and nothing about your projects changes.

**Required: at least one agent CLI, installed and signed in with your own account.**
Claude Code is the one Workbench integrates most closely (permission requests, Review
Changes, the MCP tools); Codex, Kimi Code, Gemini CLI and Aider are presets, and any other
CLI can be added in `config.toml`.

## What you need

- Linux on x86_64 (glibc 2.35 or newer for the release binaries, e.g. Ubuntu 22.04,
  Debian 12, Fedora 36). Workbench is developed and tested there. Windows 10 (version
  1809 or newer) and 11 on x86_64 are experimental: see
  [Install on Windows](#install-on-windows-experimental).
- git. To build from source: [Rust](https://rustup.rs) 1.97 or newer and
  [Node.js](https://nodejs.org) 22.
- Optional, for the features that use them: Docker (dev containers and Services),
  language servers (code intelligence), GDB / lldb-dap / debugpy / delve (debugging).

## Install a release

Download `workbench-<version>-x86_64-unknown-linux-gnu.tar.gz` from the repository's
releases, check it against its `.sha256` file if you like, and run its installer:

```bash
sha256sum -c workbench-*-x86_64-unknown-linux-gnu.tar.gz.sha256
tar xzf workbench-*-x86_64-unknown-linux-gnu.tar.gz
./workbench-*-x86_64-unknown-linux-gnu/install.sh
```

`install.sh` copies the binary to `~/.local/bin/workbench` (`PREFIX=/usr/local` for
`/usr/local/bin`, with the rights to write there) and tells you when that folder is not on
your PATH or when a running service needs a restart.

## Build and install from source

```bash
git clone <your copy of this repository> ~/src/workbench
cd ~/src/workbench
cd web && npm ci && npm run build && cd ..
cd server && cargo build --release
install -m 0755 target/release/workbench ~/.local/bin/
```

The web UI is embedded in the binary, so `~/.local/bin/workbench` is all you need to run.
To update later, install the new release (or pull, rebuild both parts and install the
binary again); then restart the service (below) or the running `workbench serve`.

## Install on Windows (experimental)

The Windows port is in progress ([status](windows-port.md)). It has not been tested on a
real Windows machine yet, and some features are left out (below). It needs Windows 10
version 1809 or newer, or Windows 11, on x86_64.

A release that includes `workbench-<version>-x86_64-pc-windows-msvc.zip` installs from
PowerShell, in the folder you downloaded it to (put the release's version in the names):

```powershell
(Get-FileHash .\workbench-<version>-x86_64-pc-windows-msvc.zip).Hash   # compare with the .sha256 file
Unblock-File .\workbench-<version>-x86_64-pc-windows-msvc.zip
Expand-Archive .\workbench-<version>-x86_64-pc-windows-msvc.zip -DestinationPath .
powershell -ExecutionPolicy Bypass -File .\workbench-<version>-x86_64-pc-windows-msvc\install.ps1
```

- `Unblock-File` removes the Mark of the Web that Windows puts on downloads, so the
  unpacked files do not carry it. Windows PowerShell runs no scripts under its default
  policy, and under RemoteSigned none downloaded unsigned: `-ExecutionPolicy Bypass` allows
  `install.ps1` for this one run.
- `install.ps1` copies `workbench.exe`, `conpty.dll` and `OpenConsole.exe` (the console host
  its terminals use), `workbenchw.exe` when the archive has it, and the documents to
  `%LOCALAPPDATA%\Programs\Workbench` without administrator rights, and adds that folder to
  your user PATH. `-Prefix <folder>` installs elsewhere. A folder it creates admits only you,
  SYSTEM and Administrators; it warns when an existing one lets other accounts change it,
  since they could then replace the programs you start from it.
- Installing over a running Workbench works: Windows cannot replace a running program, so
  its files are renamed aside (`*.old`, removed by the next install). Restart Workbench to
  use the new version.

Open a new terminal (it gets the new PATH) and start Workbench as on Linux:

```powershell
workbench serve --open
```

Where things are on Windows (`WORKBENCH_CONFIG_DIR` and `WORKBENCH_DATA_DIR` still override
both folders):

| What | Where |
| --- | --- |
| `config.toml` and the project overlays (`projects\<id>.toml`) | `%APPDATA%\workbench` |
| Workbench's state: token, sessions, Local History, Workspace cards | `%LOCALAPPDATA%\workbench`, readable only by you and SYSTEM |
| `~` in the configuration | your user folder, `%USERPROFILE%` |
| Projects found on the first start | git repositories directly under `%USERPROFILE%\workspace` |
| Token files found on the first start | `.gitlab_token`, `.github_token`, `.atlassian_token` in `%USERPROFILE%` |

Good to know:

- Terminals start PowerShell 7 (`pwsh`) when it is installed, else Windows PowerShell.
  Run configurations, pre-launch steps and the notify command run through the same
  PowerShell, so a command written for bash needs a PowerShell form (Windows PowerShell 5.1
  has no `&&`: install PowerShell 7).
- Version control needs [Git for Windows](https://git-scm.com/download/win).
- A `keyring` secret reference reads Windows Credential Manager: `{ keyring =
  "workbench/atlassian" }` is the generic credential named `atlassian.workbench`.
- The binaries are not code-signed, so SmartScreen or an antivirus may warn about
  `workbench.exe`, a program that starts terminals and other programs.
- Binding an address other than loopback (remote access) makes Windows Firewall ask whether
  to allow it.
- Not in the first version: dev containers, desktop notifications (push notifications to
  your phone are not affected), projects on WSL or network (`\\server\share`) paths, and
  gdb's attach hints and rust-gdb pretty printers (attach with debugpy, CodeLLDB or
  lldb-dap). The Services window (Docker) is untested there.

Building from source on Windows needs Rust with the MSVC toolchain (the Visual Studio Build
Tools' C++ workload) and Node.js 22. In PowerShell, from the repository (build in `server`,
where `.cargo\config.toml` links the C runtime statically):

```powershell
cd web; npm ci; npm run build; cd ..\server
cargo build --release
```

Copy `server\target\release\workbench.exe` to a folder only you can change, and put
`conpty.dll` (`runtimes\win-x64\native`) and `OpenConsole.exe`
(`build\native\runtimes\x64`) from the
[Microsoft.Windows.Console.ConPTY](https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY)
package (a `.nupkg` is a zip) next to it; the release archives use version 1.24.260710001.
Workbench loads `conpty.dll` only from its own folder or System32, never from the folder you
start it in. Without the two files, terminals use the console host built into Windows, which
renders less well on Windows 10.

## First start

```bash
workbench serve --open
```

The first start writes `~/.config/workbench/config.toml` (on Windows
`%APPDATA%\workbench\config.toml`, with `~` your user folder) from what it finds:

- **Projects:** every git repository directly under `~/workspace`. Add other folders in
  Settings › Projects, or with `include` and `exclude` in `[projects]`.
- **GitLab:** a token in `~/.gitlab_token` becomes the secret reference
  `gitlab = { file = "~/.gitlab_token" }`.
- **GitHub:** `~/.github_token` likewise. Without one, public repositories work read-only.
- **Atlassian:** `~/.atlassian_token`; set `[atlassian] site = "https://<you>.atlassian.net"`
  and your email in Settings › Integrations.

`--open` opens a browser window that is already signed in. It never puts the master token
on a command line: it asks the running server for a one-time code instead. Later, run
`workbench open` to open another signed-in window, or `workbench url` to print a sign-in
link to paste yourself.

Workbench keeps its own state (sessions, Local History, Workspace cards, scratch files) in
`~/.local/share/workbench`, on Windows in `%LOCALAPPDATA%\workbench`. Edits you make to
`config.toml` by hand apply without a restart.

## Connect an agent

Open **Agents** (the robot in the left stripe, or Ctrl+Shift+A). Pick a provider, type a
task or leave it empty for an interactive session, and press **Start session**. The session
runs in a real terminal in the project folder; Workbench adds hooks and an MCP server so
the agent can open files for you, read CI logs and Confluence pages, and hand back
Workspace cards.

Try: “Look at the failing tests in the latest pipeline and fix the first one. Open the
file you change in Workbench.”

A Claude Code session that asks for permission shows **Allow**, **For session** and
**Deny** on its card, its tab and a toast; its own prompt in the terminal keeps working,
and the first answer wins. See [working with agents](agents.md).

## A short tour

- **The stripes** on the left, right and bottom open tool windows: Files, Commit, Agents,
  Find and Workspace on the left; GitLab, GitHub, Confluence, Jira, Apps and Database on
  the right; Terminal, Problems, TODO, Git Log, Debug, Run and Services at the bottom.
- **The command palette** (Ctrl+K) lists every action; **Search Everywhere** (double
  Shift) finds files, symbols, text and actions.
- **The top bar** switches projects and branches and starts run configurations.
- **The status bar** shows the language server, the branch, CI status and the dev
  container.
- **The Workspace** (left stripe) starts with four example cards in Home: a welcome
  guide, a tour in screenshots, a report template for agents and a checklist for
  connecting your services. They stay until you archive them.

![The Services window with a compose project, next to its compose file](assets/services.png)

## Run it as a service

```bash
workbench service install --enable    # a systemd --user unit and a desktop launcher
workbench service status
workbench service uninstall
```

`install` without `--enable` only writes the files and prints the next steps
(`--dry-run` shows them first). The launcher runs `workbench open`.

## Next steps

- [Customization](customization.md): project config, run configurations, environments,
  data sources, language servers and secrets.
- [Working with agents](agents.md): providers, permissions, Review Changes, Workspace cards.
- [Your phone and remote access](remote-access.md): pairing, TLS and push notifications.
- [Security and privacy](security.md): what Workbench trusts and what it never does.
