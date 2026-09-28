# Workbench

Workbench is an AI-centric developer workspace in one window, locally or from your phone. Agent sessions (Claude Code, Codex, Kimi Code or any CLI you configure) sit at the center, surrounded by:
- the editor;
- CLion-style version control;
- GitLab and GitHub (merge and pull requests, pipelines and Actions, issues);
- Confluence and Jira;
- Workspace cards: the reports, images, PDFs and 3D models you and your agents produce, shown sandboxed;
- run configurations;
- your deployed apps.

## Run it

```bash
cd web && npm ci && npm run build && cd ..
cd server && cargo build --release          # one binary, UI embedded
./target/release/workbench serve --open     # or: workbench open (for a server already running)
./target/release/workbench service install --enable   # optional: start it with your session
```

The first start writes `~/.config/workbench/config.toml` with what it finds on this machine:
- projects: every git repo directly under `~/workspace`;
- GitLab: from `~/.gitlab_token`;
- GitHub: from `~/.github_token` (without one, public repositories work read-only);
- Atlassian: from `~/.atlassian_token`. Set `[atlassian] site = "https://<you>.atlassian.net"`.

The config holds secret *references* only. Settings → Secrets shows each one's status and can fix file permissions. Settings → Integrations sets up GitLab, GitHub (or GitHub Enterprise) and Atlassian; edits you make to `config.toml` by hand apply without a restart.

Agents: Claude Code works out of the box. Codex and Kimi Code are built-in presets once installed; any other CLI is an `[agents.providers.<name>] command = "…"` entry in `config.toml`.

## Per project

Workbench detects a project's setup from its files: run configurations from Cargo, package.json, Python, Go, CMake, Gradle and Maven, Ruby, PHP, Elixir, Make/just/Taskfile/Procfile, compose files, Unity and .NET, and environments from the Caddyfile and deploy scripts.

You can adjust it in two places:
- **`~/.config/workbench/projects/<id>.toml`** (machine-local). It can set hosts, secrets, deploys and agent settings. A project's id stays with its directory (`~/.local/share/workbench/project-ids.json`), so an overlay never moves to another repository.
- **`.workbench.toml`** in the repo (committable). Repository config is untrusted: it can never define secrets, loosen agent permissions or make Workbench run commands by itself.

## Dev containers

A project with a `devcontainer.json` (`.devcontainer/devcontainer.json`, `.devcontainer.json` or `.devcontainer/<name>/devcontainer.json`) gets a **Dev container** chip in the top bar. Its panel shows what the config would run, with dangerous items (privileged, host network, the Docker socket, host paths, `initializeCommand`…) first. Nothing is built or started until you approve that exact plan; a change to the config, its Dockerfile or compose files asks again. Once the container runs, new shells, run configurations and (if you tick **Run in dev container**) agent sessions run inside it through `docker exec`, while the files stay on this computer. Previews and readiness follow its ports. Workbench builds image, Dockerfile and compose configs itself; configs with `features` need the [devcontainer CLI](https://github.com/devcontainers/cli) (`npm i -g @devcontainers/cli`, or `[devcontainer] cli = "…"` in `config.toml`). No config yet? **Create devcontainer.json…** in the palette proposes one from the stack (Rust + Node → the Rust image with the Node feature) for you to edit before it is written.

## Code intelligence, debugging, local history

- **Code intelligence** comes from language servers: rust-analyzer, typescript-language-server, pyright, gopls, clangd and others are presets used when installed (`[lsp.servers.<id>]` in `config.toml` sets a path or adds your own). Language servers run project code, so nothing starts until you enable code intelligence for a project: the banner over its first source file, or the status bar item (**Code Intelligence…**). Then you get diagnostics and the Problems window (Alt+6), hover, completion, Ctrl+B / Ctrl+click navigation (into the standard library and dependencies too), Find Usages (Alt+F7), Rename (Shift+F6), Reformat (Ctrl+Alt+L), quick fixes (Alt+Enter) and Go to Symbol (Ctrl+Alt+Shift+N), on CLion's keymap.
- **Debugging** speaks the Debug Adapter Protocol: GDB 14+ (`gdb -i dap`), lldb-dap, CodeLLDB, debugpy and delve. Launch configurations come from Cargo targets, CMake builds, Python run configurations and Go packages, or `[[debug]]` entries in `.workbench.toml` / the project overlay (a `pre_launch` run builds first). Click the gutter for a breakpoint (right-click for conditions and log points), then Shift+F9; F8 / F7 / Shift+F8 step, F9 resumes, Ctrl+F2 stops. The Debug tool window (Alt+5) shows threads, variables, watches, the console and breakpoints; "Ask agent about this stop" hands the stop to an agent.
- **Local History** keeps every version Workbench saves and every change it sees on disk (agents' edits are attributed to their session) for a week: **Show Local History** on a file or folder, **Recent Changes** (Alt+Shift+C), Put Label…, diff and revert. Git operations that rewrite files label it first.
- **Version control** adds CLion's line staging and partial commits, changelists, the shelf, interactive rebase and bisect. **Confluence** pages get inline comments, attachments, mentions, page links, labels and page operations; Jira boards show sprints and move cards through transitions.

## Agents asking for permission

When a Claude Code session asks for permission, Workbench shows the request with **Allow**, **For session** and **Deny** on the session's card, its tab, a toast and the phone's Agents tab; Claude's own prompt in the terminal keeps working and the first answer wins (`[agents] answer_permissions`, `permission_wait`). Gemini CLI and Aider are built-in presets besides Codex and Kimi Code.

## Phone app and push notifications

Open Workbench on your phone (see Remote access) and add it to the home screen: it runs as an app. **Settings → Notifications → Push** (or the phone's More tab) turns on notifications that reach the phone while Workbench is closed: an agent that needs you, with **Allow** / **Deny** right on the notification when the request is short enough to read there, a finished turn, an environment going down, a failed deploy or pipeline.

Push needs **HTTPS**: browsers only offer it to secure pages (`http://localhost` counts, a LAN or Tailscale IP over plain http does not). Use `tailscale serve`, Caddy or `[server.tls]`. On iPhone and iPad, push works only in the Home Screen app (iOS 16.4+). Notifications go through the browsers' own push services (Google, Mozilla, Apple, Microsoft), end-to-end encrypted, with titles and short summaries only.

## Run it as a service

```bash
workbench service install --enable   # systemd --user unit + a desktop launcher, started now
workbench service status
workbench service uninstall
```

`install` without `--enable` only writes the files and prints the next steps (`--dry-run` shows them first). The launcher runs `workbench open`, which signs the browser in with a one-time code (never the token on a command line) and keeps its existing session.

## Remote access

Bind beyond loopback, e.g. `bind = "100.x.y.z:7777"` for your Tailscale address; the local listener keeps working. Then pair a phone from **Settings → Remote access** with the QR code. Every device can be revoked, and revoking closes its open connections.

Put TLS in front with `tailscale serve` or Caddy, or set `[server.tls]`.

Claude Code's own Remote Control can be switched on per session, or run as a server per project, from the Agents panel.

## Developing Workbench in a dev container

`.devcontainer/` has everything needed to work on Workbench itself: Rust, Node 22, the C toolchain for rustls, the tools the tests use, and Claude Code. Open it with VS Code (*Reopen in Container*), with `devcontainer up --workspace-folder .`, or from Workbench.

Build output and the cargo registry live in named volumes. Claude Code's login is kept per container, so your host `~/.claude` is not shared.

Inside the container, use port 7787 so it doesn't clash with a Workbench on the host:

```bash
cd server && cargo run -- serve --bind 0.0.0.0:7787
cd web && npm run build          # or: npm run dev -- --host 0.0.0.0   (port 5173)
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design, contracts and security model.

## License

MIT — see [LICENSE](LICENSE). The Workspace feature adapts code from Mr. Mak Workspace (MIT); see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
