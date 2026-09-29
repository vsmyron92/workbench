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
  Debian 12, Fedora 36). Workbench is developed and tested there; Windows is not
  supported yet.
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

## First start

```bash
workbench serve --open
```

The first start writes `~/.config/workbench/config.toml` from what it finds:

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

Workbench keeps its own state in `~/.local/share/workbench` (sessions, Local History,
Workspace cards, scratch files). Edits you make to `config.toml` by hand apply without a
restart.

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

## On Windows

The Windows version (in progress: the [Windows plan](windows-port.md)) leaves out a few
things; where one of them is asked for, Workbench says "Not available on Windows" and why:

- **Dev containers.** Their chip, status item and commands are not shown. The **Services**
  window (Docker containers, compose projects, images) works with Docker Desktop but is
  marked *experimental*: it has not been tested there yet.
- **Desktop notifications** from the server. Turn on browser notifications
  (Settings › General), which then also notify on the computer Workbench runs on, or push.
- **gdb attaching to a running process**, and rust-gdb's pretty printers. Attach to
  Process… uses lldb-dap or CodeLLDB for native programs and debugpy for Python.
- **Projects on a network share or inside WSL** (`\\server\share`, `\\wsl$\…`). Clone the
  repository to a local drive, or run the Linux Workbench inside WSL for those projects.

## Next steps

- [Customization](customization.md): project config, run configurations, environments,
  data sources, language servers and secrets.
- [Working with agents](agents.md): providers, permissions, Review Changes, Workspace cards.
- [Your phone and remote access](remote-access.md): pairing, TLS and push notifications.
- [Security and privacy](security.md): what Workbench trusts and what it never does.
