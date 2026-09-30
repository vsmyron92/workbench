# Workbench architecture

Workbench is an AI-centric developer workspace. Claude Code sessions sit at the center, and everything you or an agent needs to see is around them:
- code, diffs and git history;
- CI pipelines and merge requests;
- Confluence and Jira;
- the running local, staging and production apps.

The goal is to stop switching between CLion, Atlassian and GitLab tabs. It is a generalization of Mr. Mak Workspace (Windows, Tauri + Node) to any project, running on Linux first and usable remotely from a phone.

## Process model

```
browser (desktop app window via chrome --app, or a phone)
   │  HTTPS/HTTP, cookie auth, one events WebSocket + one WebSocket per open terminal
   ▼
workbench (single Rust binary, axum)
   ├─ serves the React SPA (embedded in release; read from web/dist in debug)
   ├─ REST  /api/**      ── feature slices
   ├─ WS    /api/events/ws   (server → UI events)
   ├─ WS    /api/terminals/{id}/ws   (PTY I/O)
   ├─ MCP   /mcp          (hosted Claude sessions drive Workbench: open files, read Confluence, CI logs…)
   ├─ hooks /api/hooks/** (Claude Code HTTP hooks → precise agent state, held permission requests)
   ├─ /sw.js, /manifest.webmanifest (installable app, Web Push to the browsers' push services)
   ├─ language servers (stdio) and debug adapters (DAP) per project, started only on the user's say-so
   └─ PTYs: claude, shells, run configurations, deploy/log commands, debuggees
```

Why Rust:
- It is one small, fast binary with no runtime to install.
- PTY fan-out, file watching, ripgrep-speed search and many concurrent HTTP integrations are all comfortable in tokio.
- Axum + React is a common, well-supported pairing.

Python would have been fine for I/O, but Rust is lower overhead and fits better.

**Remote control has two layers:**
1. **Workbench itself.** Bind it to a LAN or Tailscale address and pair devices. The phone UI is a separate `MobileShell`.
2. **Claude Code Remote Control.** Sessions can start with `--remote-control`. Their claude.ai link is shown per session, and `claude remote-control` servers can run per project.

**Local helpers always reach `http://127.0.0.1:<port>`** (`AppState::local_base_url`): agent hooks, the MCP URL in each session's settings, `WORKBENCH_URL`, git askpass and the rebase editor helper, the statusline helper and `workbench open`. When `server.bind` does not cover 127.0.0.1 (just a Tailscale or LAN address, `[::1]`, `127.0.0.2`), `main.rs` also listens on `127.0.0.1:<port>` with plain HTTP (`app::loopback_listen_addr`) and refuses to start if that port is taken.

## Repository layout

```
server/            Rust crate `workbench`
  src/main.rs        CLI: serve | open | url | askpass | git-editor | statusline | service
                     (and git/ssh's askpass call on Windows: WORKBENCH_HELPER=askpass <prompt>)
  src/bin/workbenchw.rs  Windows launcher behind `workbench service` (GUI subsystem; a stub elsewhere)
  src/app.rs         AppState + router assembly (core)
  src/auth.rs        token → device cookie, pairing, Host/Origin guard, agent tokens (core)
  src/config/        global config.toml + project model (layered TOML) (core)
  src/projects.rs    project registry (core)
  src/events.rs      event bus + /api/events/ws (core)
  src/secrets.rs     secret references → values (core)
  src/mcp.rs         MCP tool type + in-process REST dispatch (core)
  src/util/          atomic writes, path containment, process runs, ANSI strip, git helpers (core)
  src/util/os/       the operating-system layer: Unix and Windows bodies behind one interface (core)
  src/terminals/     SLICE terminals: PTYs, agents, hooks, history, remote control, permission requests
  src/files/         SLICE files: tree, read/write, watch, search, quick open, local history (history/)
  src/git/           SLICE git: CLion-style VCS, line staging, interactive rebase, changelists, shelf, bisect
  src/lsp/           SLICE lsp: language servers → Monaco (process manager, trust gate, URI mapping, editor socket)
  src/debug/         SLICE debug: Debug Adapter Protocol sessions, launch configurations, breakpoints
  src/gitlab/        SLICE gitlab: MRs, pipelines, jobs, envs, issues, registry
  src/github/        SLICE github: PRs, Actions runs/jobs/logs, issues, releases
  src/workspace/     SLICE workspace: Mr. Mak-style deliverable cards and Home examples, sandboxed report serving, trash
  src/forge.rs       core: GitLab/GitHub dispatch (commit_ci_status) for forge-agnostic features
  src/atlassian/     SLICE atlassian: Confluence + Jira
  src/apps/          SLICE apps: run configurations, environments, auto-detection
  src/db/            SLICE db: the Database tool window's PostgreSQL data sources, schema browsing, SQL consoles
  src/devcontainer/  SLICE devcontainer: devcontainer.json review, engines, terminals and runs inside, bridge listener
  src/platform/      SLICE platform: MCP server, remote access, settings, notifications, Web Push (push/), service install
  src/spa.rs         core: the embedded SPA, its CSP, /sw.js and /manifest.webmanifest
  .cargo/config.toml Windows (MSVC) builds link the C runtime statically
web/               React 19 + TS + Vite 8
  public/            manifest.webmanifest, sw.js (service worker), icons/ (from scripts/icons.mjs)
  src/api/           fetch client, events socket, shared types, shared queries (core)
  src/shell/         desktop + mobile shells, dock, palette, dialogs, feature registry (core)
  src/ui/            shared components (core)
  src/theme/         tokens.css (design tokens), palette.ts (Monaco/xterm colours) (core)
  src/lib/           monacoSetup, languages.ts (file → Monaco language), vhdl.ts (grammar) (core)
  src/features/<slice>/   one folder per slice; index.ts exports a FeatureModule
docs/              this file
packaging/linux/   install.sh shipped in the Linux release archive
packaging/windows/ install.ps1 and CONPTY_NOTICE.md, shipped in the Windows release archive; workbench.ico (from web/scripts/icons.mjs) and workbench.manifest, embedded in the Windows executables
.github/workflows/ ci.yml (web and server build + tests, Linux and Windows), release.yml (tag → Linux archive, Windows zip when `RELEASE_WINDOWS` is set, + GitHub release)
```

**Ownership rule.** A slice owns `server/src/<slice>/**` and `web/src/features/<slice>/**`. Core files change only when the contract changes. `Cargo.toml` and `package.json` already list everything a slice is expected to need; adding a dependency is allowed but should be rare.

## Configuration

- `~/.config/workbench/config.toml` (`%APPDATA%\workbench\config.toml` on Windows) holds global settings (`config/global.rs`). It is written with detected defaults on first run.
  - It contains `[server]` (bind, allowed_hosts, public_url, tls), `[projects]` (roots, include, exclude), `[agents]` defaults (with `answer_permissions` and `permission_wait`) and `[agents.providers.*]`, `[terminals]` (`shell`: the argv of new shells; default `$SHELL -l`, on Windows PowerShell), `[gitlab]`, `[github]`, `[atlassian]`, `[notify]`, `[push]` (subject, extra_endpoint_hosts), `[lsp]` (`idle_minutes`, `[lsp.servers.*]`), `[debug]` (`default_adapter`, `[debug.adapters.*]`), `[devcontainer]` (docker, cli, engine), `extra_roots` and `[secrets]`.
  - Settings saves edit config.toml in place (`platform::config_edit`): comments and layout survive, for every section.
  - Settings saves apply at once. Edits made outside Workbench (an editor, a setup hint followed by hand) apply too: `platform::settings::watch_config` watches the config directory and applies a valid `config.toml` like a raw save (config swapped, secret cache cleared, projects reloaded, `settings.changed`). A file that does not parse or fails the hard checks is reported once (`ui.notify`) and the running config stays; the watcher never writes the file.
- **Project ids** are the directory name as a slug (`api`, then `api-2`… for another directory of that name) and are bound to the directory for good in `data_dir/project-ids.json` (canonical path → id). Everything keyed by an id belongs to that directory: the overlay `projects/<id>.toml` with its secrets, `data_dir/workspace/<id>`, terminals and agent sessions (`projectId`, hence their MCP confinement). Scan order (roots, then includes) only decides the id the first time a directory is seen; adding a root with a same-named repository or removing the first of two never moves an id, and a new directory never gets an id the file gives to another one, even one that is gone or excluded. A moved repository therefore gets a new id: rename its overlay to follow it.
- **Scratch files** (CLion's Scratches) are the hidden project `wb-scratches` (`projects::SCRATCH_ID`, a reserved id like `home` and `all`): the folder `data_dir/scratches`, with no detection or config layers. It is never listed (`ProjectRegistry::list`, so `/api/projects`, the switcher, pollers and settings see only real projects), but `get` / `require` and `find_by_path` reach it (and the file watchers, via `list_with_scratches`), so every `/api/projects/wb-scratches/files/**` route works: editor buffers, Local History, the HTTP Client (env files next to the scratches). `resolve_in_root` keeps them inside the folder, away from the data dir's token and sessions. The UI (files slice, `scratches.ts`, `scratchStore.ts`): the Files tool window's Scratches view (its notebook button; per browser; Locate follows the file shown), **New Scratch File…** (Ctrl+Alt+Shift+Insert: a language popup, then `scratch.<ext>`, `scratch_2.<ext>`… created with `etag: null` so it never overwrites), Show Scratch Files, and a Scratches group in Recent Files (Ctrl+E). No git queries are made for them.
- The project model (`config/project.rs`) merges three layers, lowest priority first:
  1. `apps::detect(root)`, which is never written to disk;
  2. `<repo>/.workbench.toml`, which can be committed and holds no secrets;
  3. `~/.config/workbench/projects/<id>.toml`, a machine-local overlay for hosts and secret references.
- `[[run]]`, `[[env]]`, `[[component]]`, `[[debug]]` (launch configurations) and `[[database]]` (data sources) entries merge by `name`; a later layer replaces the whole entry.
- **Repository config is untrusted.** Layers 1 and 2 come from repository content (a third-party clone, someone else's branch). `config::project::merge_layers` applies these rules before merging them, and every removal becomes a project warning:
  - `[secrets]` is ignored: secret references (`command`, `file`, `dotenv`…) are read only from the overlay and config.toml.
  - Secret *names* used by repository entries (env `auth.password`, `repo.gitlab.token`, `links.*.token`, `database.password` / `database.url`) resolve only against the overlay's `[secrets]`, never config.toml's (`Project::repo_secret_names`, enforced in `AppState::secret`).
  - `agent.env`, `agent.add_dirs` and any `agent.permission_mode` other than `manual`, `plan` or `dontAsk` are ignored.
  - Language servers and debug adapters are commands: only config.toml defines them (`[lsp.servers.*]`, `[debug.adapters.*]`); a repository's `[[debug]]` launch configuration names an adapter by id, never a command, and its `${secret:…}` env resolves only against the overlay.
  - Nothing runs by itself: a service run's `status` (polled in the background) and an env health probe over ssh (`via_host`, unless its host is an overlay `[hosts]` entry) are dropped.
  - A token-less `links.confluence`/`links.jira` keeps its `site` only when it is the configured site or an `https://*.atlassian.net` site, so config.toml's Atlassian token never goes elsewhere.
  - A token-less `repo.gitlab` gets config.toml's `[gitlab]` token only when its base URL, **scheme included**, is the configured host (`gitlab::client::global_host_matches`), so an `http://` twin of an https host never receives it.
  - Commands that run on a click (runs, deploys, env logs and commands) stay: like any IDE's run configurations, the owner sees and starts them.
  - An overlay entry with the same name replaces the repository's entry, and with it these restrictions. `git::askpass` applies the same rules (`load_layers`).
- **Secrets:** config holds references only, e.g. `gitlab = { file = "~/.gitlab_token" }`, or `env`, `keyring`, `dotenv` or `command`.
  - Values are resolved in the backend (`AppState::secret(project, name)`).
  - They never go to the browser, never go into argv, and are never logged.
  - Text streamed to the UI can be passed through `secrets::redact`.
  - A machine overlay that does not parse is left out whole (a project warning). A secret missing for that reason answers with the overlay's error in one line (`Project::overlay_error`), not with advice to add it there.
- **Environment overrides:**
  - `WORKBENCH_CONFIG_DIR` and `WORKBENCH_DATA_DIR` isolate instances. Every test or dev run that is not the owner's real instance must set both.
  - `WORKBENCH_LOG` sets the tracing filter.

Data dir (`~/.local/share/workbench/`; on Windows `%LOCALAPPDATA%\workbench`, apart from the roaming config in `%APPDATA%`), all files mode 0600 (on Windows a protected DACL for the user and SYSTEM only, set at creation and passed on by the data dir to everything inside; `util::os::perm`):
- `token`: the master token.
- `auth.json`: device sessions, stored as SHA-256 hashes.
- `runtime.json`: pid and URL of the running server.
- `project-ids.json`: which directory each project id belongs to (see Configuration).
- `scratches/` (0700): scratch files (see "Scratch files").
- Slice state such as terminal metadata and screens (`terminals/`) also lives here, and `devcontainer/<id>.json`: dev container approvals (sha256 of approved plans), settings and the last start.
- Third phase: `lsp/<id>.json` (code intelligence enabled for that directory), `debug/<project>.json` (breakpoints, watches) and `debug/adapter/` (the adapters' working directory), `push/vapid.json` and `push/subscriptions.json`, `local-history/<project>/` (index and zstd blobs), `git/changelists/<project>.json`, `git/shelf/<project>/`, `git/rebase/` (a running interactive rebase's staging), `workspace-trash/<scope>/`.

## Security model

- **Host pinning:** requests are accepted only for `localhost` and loopback IP literals, the bind IP, `server.allowed_hosts` and the `public_url` host (`auth::host_accepted`, also used by Settings). A DNS name that merely starts with `127.` is not loopback. This defends against DNS rebinding.
- **Auth:**
  - `/api/**` and `/mcp` need a device cookie (`wb_session_<port>`, HttpOnly, SameSite=Strict) or `Authorization: Bearer <master token>`.
  - The login URL `/auth?token=…` swaps the token for a cookie and redirects.
  - Pairing uses `POST /api/auth/pair`, which gives a 10-minute one-time code redeemed at `/pair?code=…`.
  - `workbench open` (and the desktop launcher, `serve --open`) never put the master token on a browser's command line, where every local process could read it (`/proc/<pid>/cmdline`): it asks the running server for a one-time code (`POST /api/auth/pair {launch: true}` with the token in the `Authorization` header; only the master token mints launch codes) and opens `/pair?code=…`. `workbench url` still prints a token URL for the user to copy.
  - **A browser that signs in again keeps its session.** When `/auth` or `/pair` is opened with a live session cookie of this port (the launcher, clicked every day), the session stays (same id, name and push subscription; the cookie's lifetime is renewed) and gains a new device key in the fragment instead of a new session per launch. The session's earlier keys keep working (`DeviceSession.other_keys`, up to 8, least recently used dropped first; use is recorded at most once a minute), so the browser's open tabs stay signed in; the page keeps its stored key while it works (`settleDeviceKey`).
- **Device keys:** browsers send 127.0.0.1 cookies to *every* local port, so any other local server the browser talks to (a dev server, an app preview) receives the cookie.
  - A cookie alone therefore only reads (GET). Non-GET requests and WebSocket upgrades also need the session's device key: header `X-Workbench-Key`, or `?wbk=` on WebSocket URLs.
  - The key is handed over once at sign-in: `/auth` and `/pair` redirect to `/#wbk=<key>` (a URL fragment never reaches a server; the SPA stores it in its origin's `localStorage` and removes it from the URL), and `POST /api/auth/login` returns `{ok, key}`. Any page can open `/#wbk=junk`, so a handed-over key replaces a stored one only when the stored key no longer signs the browser in (`settleDeviceKey`, checked with `GET /api/auth/status` before the first request).
  - `api/client.ts` adds the key to every request and `wsUrl()`. `GET /api/auth/status` reports `authenticated` only with the key, so a browser without it (or a session from before device keys) signs in again.
- **Ending a session** (revoke in Settings, logout, 30-day expiry) also closes that device's open WebSockets with close code `4401`: socket loops select on `state.auth.watch(caller).ended()`. The SPA then shows the login screen.
- **CSRF:** cookie-authenticated non-GET requests and WebSocket upgrades must be same-origin (`Origin` host equals `Host`).
- **Content-Security-Policy** on the SPA document (`spa::CSP`): only the app's own scripts run; `object-src 'none'`, `frame-ancestors 'none'`.
- **Rendered Markdown is untrusted** (`ui/Markdown.tsx`). Raw HTML goes through `rehype-sanitize` with GitHub's schema: no scripts, iframes, event handlers, `style` or `javascript:` URLs, and ids and names from the document are prefixed `user-content-` so they cannot clobber the app's globals. Heading ids (`md-`) and alert classes are added after sanitizing. Mermaid diagrams render with `securityLevel: 'strict'`. The CSP is the second line: nothing inline can run.
- **Logs:** request spans carry the method and path only, never the query string (`/auth?token=…`, `?wbk=`).
- **Agent tokens:** `auth.issue_agent_token(terminal_id)` is put into a hosted session's environment (`WORKBENCH_AGENT_TOKEN`). It is valid only on `/api/hooks/**` and `/mcp`, whose handlers check it with `auth.agent_from_headers`.
  - A hosted session is confined to its own project over MCP. Tools resolve the project with `McpCtx::project_for`, which refuses a `projectId` naming another project; terminal tools show only what `McpCtx::may_see_project` allows. Only a caller that is not a session (the master token without `X-Workbench-Terminal`) may name any project.
- **Git credentials** (`git::askpass`): remote ops (fetch, pull, push, rebase) let git ask `workbench askpass`, which answers only for the GitLab host Workbench has a token for (the project's `[repo.gitlab]` with its own token, else `[gitlab]`), only over https, and only when the prompt names that host unambiguously: git before its CVE-2024-50349 fix prints user names decoded, so `Password for 'https://gitlab.com/@evil.example': ` (user `gitlab.com/`) is refused. For that host the ops also empty git's credential helper list (`-c credential.https://<host>.helper=`, `askpass::reset_helpers_key`), so no helper (Git Credential Manager, `store`, `cache`, a keychain) is asked for it or handed the token to store; other hosts keep the user's helpers.
- **Paths:** every client path goes through `util::paths::resolve_in_root`, or through `resolve_absolute_in` for extra roots. These reject `..` escapes and symlinks leaving the root.
  - Whether a string is an absolute path, and the Windows rules, live in `util::os::path`. On Windows client paths use `/` only (a `\` could slip past checks that split on `/`), and names that alias another file or a device are refused (`:`, device names like `NUL` or `com1.txt`, a trailing dot or space, 8.3 short names like `GIT~1`); roots compare without regard to ASCII case; UNC roots (`\\server\share`, `\\wsl$`, also spelled `\\?\UNC\…` or `\??\UNC\…`) are refused: adding one as a project answers `unsupported_platform` (`networkRoots`), so does a path that resolves to one (a mapped network drive), and config.toml entries naming one are skipped at reload before anything opens them. Canonical paths drop `\\?\` wherever a plain path names the same file and have an uppercase drive letter; comparisons take `\\?\C:\` for `C:\`. Linux keeps its rules: a `\` is part of a name, in the paths clients send and in the relative paths the server answers with, which `os::path::to_slash` writes with `/` on Windows only (`util::paths::relative_to`, listings, search, quick open, the watcher).
  - **Links to other computers are never followed** (Windows): opening a link to `\\host\share` signs in to that host with the user's credentials, and repositories can hold such links. `os::path::canonicalize` resolves links one at a time, reading each target before following it, and refuses UNC, device and NT targets (`is_refused_link`: `resolve_in_root` answers 403, a listing shows a broken link); `os::path::leaves_machine` / `leaves_machine_below` check a path before Workbench opens it by itself (detection, `.workbench.toml`, ignore files and walks, the watcher, run and debug configurations, the project's MCP files, the projects under a root). Linux follows links as before.
- **PTY input is code execution.** Every authenticated device is fully trusted, so remote exposure requires pairing and should use TLS (a proxy such as `tailscale serve` or Caddy, or `[server.tls]`).
- **Dev containers:** a `devcontainer.json` (with its Dockerfile and compose files) is repository content that runs code on the host's Docker. Nothing builds or starts without the user's approval of the exact plan (a sha256 the server checks); agents can only read the status. The bridge listener on a container network's gateway serves only `/api/hooks/**` and `/mcp`, only with agent tokens. See "Dev containers".
- **Docker (Services)** is root on this computer: `/api/docker/**` acts only for devices (agent tokens are not valid there; in-process callers get 403 on every route that changes something or opens a terminal). Details and `inspect` mask values of secret-looking names (`*PASSWORD*`, `*TOKEN*`, `*_KEY`, `*SECRET*`…), passwords in URLs, and those inside JSON labels (`devcontainer.metadata`'s `remoteEnv`).
- **Databases** (`/api/projects/{pid}/db/**`): a console runs what the user types with the database user's rights, so everything but the list refuses in-process callers (agents over MCP get 403; agent tokens are not valid there at all). Credentials are secret names resolved in the backend; the list shows the names, never values; a password typed into the browser is refused. Connection errors are redacted of the secrets used.
- **Language servers and debug adapters** run project code. Their commands come only from config.toml; a project's servers start only after the user enabled code intelligence for that directory (`data_dir/lsp/<id>.json`), a debug session only on a click; debug adapters start in `data_dir/debug/adapter` so a repository's modules cannot shadow the adapter's (debugpy). Every write route of both slices refuses in-process callers: agents (MCP) read diagnostics, symbols and a stopped session's state, and never enable, start, step, evaluate or stop anything. See "Code intelligence" and "Debugger".
- **Permission requests** of Claude Code sessions are answered only by signed-in devices (`POST /api/agents/{id}/permission`: agent tokens 401, the master token and in-process calls 403), so a session never approves itself through Workbench. One-tap Allow (the attention toast, a push notification) is offered only for a request shown whole: `pendingPermission.complete` and at most 300 characters on 4 lines. See "Approvals".
- **Web Push** posts to a browser-supplied endpoint: an SSRF boundary. Only the browsers' push services (FCM, Mozilla, Apple, WNS) and `[push] extra_endpoint_hosts` are accepted, https on the default port, no IP literals, no redirects; payloads carry titles and short summaries only. The service worker answers Allow / Deny with the device key it keeps in IndexedDB (as exposed as the page's `localStorage` copy). See "Phone app and push".
- **Environment hygiene:** child processes never inherit the Claude session variables of whatever started Workbench (`util::proc::SESSION_ENV_VARS`).
- **Remote services:** never echo upstream error bodies that could contain credentials. Build URLs without userinfo.

## Contracts

### Rust

| Item | Owner | Used by |
|---|---|---|
| `Terminals::{spawn, spawn_redacted, respawn_redacted, info, list, kill, write, send_text, subscribe_output, exit_watch, screen_text}` and `SpawnSpec`, `TerminalInfo`, `AgentInfo`, `ExitInfo` | terminals | apps (runs, deploy/logs), platform (MCP), git (optional), debug (pre-launch commands, `runInTerminal` debuggees) |
| `terminals::{PendingPermission, permission_wait_secs}`, `AgentInfo.pending_permission` (the request a session waits on; `complete` and `detail` decide one-tap Allow) | terminals | platform (push: Allow / Deny, the push TTL) |
| `terminals::in_container(&TerminalInfo)` | terminals | files (local history: a dev-container agent's paths) |
| `files::history::agent_hook(state, terminal_id, payload)` (every Claude hook payload: Write/Edit attribution) | files | terminals (hook route) |
| `files::history::auto_label(state, pid, text)` ("Before git checkout main", best effort) | files | git (before every operation that rewrites the working tree) |
| `apps::runs::{start, needs_confirmation, resolve_cwd, RunState}`, `apps::expand::{run_env, base_vars, placeholders, shell_quote}` | apps | debug (a launch configuration's `pre_launch` run and its variables) |
| `apps::detect::venv_python(root, Dialect)` (the project's virtualenv interpreter, relative to its root, in that dialect's form) | apps | debug (derived Python launches, with `Dialect::HOST`) |
| `Terminals::sandboxed_agent(state, terminal_id) -> Option<String>` (the calling session runs its commands in its CLI's sandbox) | terminals | apps (`run_start` refuses such sessions) |
| `forge::commit_ci_status(state, project, sha)`, `forge::forge_of(project)` | core | apps (deploy gate), CI widgets |
| `gitlab::commit_ci_status(state, project, sha)`, `github::commit_ci_status(state, project, sha)` (same `CiStatus`) | gitlab, github | `forge` |
| `apps::detect(root) -> ProjectFile` | apps | projects registry |
| `devcontainer::{summary, running_target, exec_target, uses_container, run_inside, port_route, agent_command, container_has_curl, write_into, kill_inside, workspace_mount, require_supported, docker::exec}`, `ExecTarget::{wrap, map_path, describe}` | devcontainer | projects (`ProjectSummary.devcontainer`), terminals (`meta.inContainer`), apps (runs inside, readiness, previews), lsp and debug (servers and adapters inside, path mapping), files (`workspace_mount`: agent paths back to the host), terminals and lsp (`require_supported`: `unsupported_platform` before a shell, an agent session or a language server is asked to run inside a container) |
| `<slice>::mcp_tools() -> Vec<McpTool>` | each slice | platform (`/mcp` server via `mcp::all_tools`) |
| `<slice>::{router, start}`, `terminals::shutdown`, `apps::shutdown`, `lsp::shutdown`, `debug::shutdown` | each slice | app.rs |
| `git::{cli_askpass, cli_git_editor}`, `terminals::cli_statusline`, `platform::service::cli` | git, terminals, platform | main.rs (`askpass`, `git-editor`, `statusline`, `service`; `cli_askpass` also for `util::os::helper::askpass_prompt`) |
| `mcp::call_api(state, method, path, body, ctx)` (a route's error keeps its status and message, and its code when it is one of Workbench's own, `error::CODES`: `not_configured`, `unsupported_platform` with its `feature`…; any other is `upstream`) | core | any MCP tool that reuses a REST route |
| `McpCtx::{project_for, may_see_project}` | core | every MCP tool that takes a project or reads another terminal |
| `AppState::secret`, `events.emit/ui_open/ui_open_id/notify`, `projects.require/find_by_path` | core | everyone |
| `Project::{gitlab, github}() -> Option<(host, path)>` | core | gitlab, github, forge, apps, UI (`ProjectSummary.gitlab/github`) |
| `auth.watch(caller)` → `SessionWatch::ended`, `auth::session_ended_close()` | core | every WebSocket handler (events, terminals, lsp) |
| `auth.sessions_ended()` (a watch bumped when device sessions end), `auth.session_active(id)` | core | platform (push subscriptions end with their session) |
| `lsp::LspConfig`, `debug::DebugConfig`, `platform::push::PushConfig` | lsp, debug, platform | core config (`GlobalConfig.{lsp, debug, push}`: config.toml `[lsp]`, `[debug]`, `[push]`) |

**Terminals spawned by other slices:**
- `spawn_redacted(state, spec, secrets)` masks the given secret values (`${secret:…}` in a run's env) in the output at the source: the screen, saved screens, the WebSocket, `screen_text` and MCP never see them. On Windows a value is also masked when ConPTY's repainting puts escape sequences between its characters.
- `POST /api/terminals/{id}/restart` re-runs a run/command terminal only when that is safe without its owner: log follows (`meta.action = "logs"`), Remote Control servers, or `meta.restartable = true`. Run configurations restart through apps; deploys and env commands only from their environment, so gates and confirmation run again. Others get `409 not_restartable`.
- `TerminalInfo.lingering` counts processes the exited process left running in its session; kill, close and restart end them.
- A terminal's processes are a session of `util::os::session`, keyed by the leader's pid: a Unix session (hang-up: SIGHUP and SIGCONT to its process groups, SIGKILL after the grace period), on Windows a Job Object the leader joins right after the spawn (a process that asks to leave it may: `JOB_OBJECT_LIMIT_BREAKAWAY_OK`), in a pseudoconsole (ConPTY). There the hang-up closes the pseudoconsole (CTRL_CLOSE_EVENT to every attached process) and `TerminateJobObject` ends the rest; the pseudoconsole also closes once the leader has exited and the job is empty, since ConPTY gives no EOF of its own; `lingering` counts the job's live processes. A GUI program is a member like any other: a browser or editor that a terminal's program starts when it was not running yet ends with the terminal, where on Linux its launcher usually puts it in a session of its own (a known Windows difference, windows-port.md §1.F). `Pty::spawn` always hands portable-pty an absolute program (`util::os::exe::launch`: a relative path from the terminal's cwd, a bare name from its own `PATH` first): npm shims run as node and their script, a batch file only with a path and arguments cmd.exe reads as they are and with `NoDefaultCurrentDirectoryInExePath=1`, and an agent's initial prompt is pasted instead of passed to a batch file when it holds `% ! ^ & | < > "` or a line break. Every terminal but an interactive shell's (runs, pre-launch steps, commands, agent CLIs) gets `util::os::exe::child_env` last, so a cmd.exe among its processes never runs a program from the current directory; a shell keeps the lookup its user types commands for (in cmd.exe `build` runs the `build.bat` there). Every terminal's `PATH` is `util::os::env::fresh_path()` (`base_env`; a spec's own `PATH` wins): the one a new sign-in gets, then Workbench's own absolute entries it lacks, so a program installed since Workbench started is found (portable-pty lays the registry's `Environment` values over Workbench's environment anyway).

**Handlers:**
- Return `ApiResult<Json<T>>`. JSON is camelCase (`#[serde(rename_all = "camelCase")]`).
- Errors come from the `ApiError` constructors. Use `not_configured` for a missing token, site or similar, which makes the UI show setup help.
- A feature the OS leaves out answers `ApiError::unsupported(feature, reason)`: HTTP 501, `{error: {code: "unsupported_platform", message, feature}}`. The table of such features is `util::os::support` (`Feature`, `unsupported(f)`, `require(f)`, `require_root(path)`); everything is supported on Linux. MCP tools return the same reason.
- Use `conflict` for optimistic-concurrency failures and `upstream` for remote failures.
- A slice's own code (`ApiError::new(status, code, …)`, such as `port_in_use`) is also listed in `error::CODES`, so an MCP tool that calls the route through `mcp::call_api` gets it too; a test checks the list against the source.

### Operating-system layer (util::os)

`server/src/util/os/` (core) holds everything that differs between Linux and Windows: a file
per area with its `cfg(unix)` body (the code Workbench always had) and its `cfg(windows)`
body; `win32.rs` has the Windows helpers they share (handle and `LocalFree` guards, wide
strings, registry values). **Call sites stay free of `cfg(unix)` / `cfg(windows)`** (tests
excepted): a slice that needs something OS-specific adds it to an area here, and Linux
behaviour does not change.
The plan and its status are in [windows-port.md](windows-port.md).

| Area | What callers get | Where Windows differs |
|---|---|---|
| `perm` | private files and directories: `apply(path, mode)`, `create_dir_private`, `open_new`, `open_append`, `set_len` (use it, not `File::set_len`, on a file opened for appending), `privacy`, `owned_by_me`; `util::fs::write_atomic` sits on it | A mode without group or other bits (0600, 0700) is a protected DACL for the user and SYSTEM, set at creation and inherited inside a directory; other modes inherit the folder's ACL. `privacy` reads the DACL; a replacement gets the replaced file's DACL. An append handle may not truncate (std's `set_len` on it is access denied): `set_len` reopens it for writing. |
| `fs` | `rename_noreplace`, `rename_exchange`, symlinks, `trash`, `read_text` (a text file the user wrote), `NATIVE_CRLF`, `FOREIGN_OWNERS` | `MoveFileExW` without replacing; no atomic exchange (`rename_unsupported`, callers fall back); creating a symlink needs Developer Mode or an administrator; the Recycle Bin; `read_text` also decodes UTF-16LE and UTF-8 with a byte order mark (Windows PowerShell 5.1's files). |
| `proc` | `ProcGroup` (a child and what it starts), `pid_alive`, `kill_pid`, `exit_text`, `current_exe`, `user_processes`, `debugger_attached`, `shutdown_signal(data_dir)`, `enable_ctrl_c` | Job Objects instead of process groups: a child gets a hidden console of its own and joins a job with `KILL_ON_JOB_CLOSE`; `terminate` and `kill` both end the job (the graceful step is the protocol's: LSP exit, DAP disconnect). Process lists through sysinfo. The server stops on Ctrl-C, Ctrl-Break, the console closing or the event `Local\workbench-<hash of data_dir>` (`request_stop`; one per Windows session, `session_of`). `serve` clears the inherited "ignore Ctrl-C" first (`enable_ctrl_c`), so Ctrl-C works in its terminals however it was started. |
| `shell` | `interactive()` (terminals), `run_argv` / `run_command` (runs, pre-launch steps, service commands), `plain_command` (the notify command), `quote` for that shell, `posix_quote` for POSIX shells elsewhere (ssh hosts, containers), `helper_command` | PowerShell: `pwsh`, else Windows PowerShell. Commands run as `-NoProfile -EncodedCommand` (UTF-16LE, base64), which no argv quoting can alter, and keep the failing program's exit code (127 when not found). `readable_stderr` (what `util::proc::run_cmd` keeps) turns the CLIXML PowerShell writes its own records in on a pipe (errors, warnings, progress) into the console's text, without colour escapes. No `-OutputFormat Text`: pwsh would then write warnings to stdout. |
| `exe` | `resolve` / `which`, `is_executable`, `configured(argv)` (a config.toml command), `launch(argv, cwd, env) -> Result<Launch, String>` (a terminal's argv and what its environment gets on top; on Windows `Err` when the program is not found or is a batch file cmd.exe would misread), `python()`, `rustup_proxy`, `child_env`, `program_env` (what a program the server starts by itself gets), `INSTALLED_SINCE` (the end of a "not found on PATH" message) | Lookup over `PATH` × `PATHEXT`, then `%USERPROFILE%\.local\bin` and `%APPDATA%\npm`, never the current directory. An npm `.cmd` shim starts as `node.exe <script>` (`Kind::NpmShim`); another batch file only when `batch_args_safe` (BatBadBut). `child_env` (`NoDefaultCurrentDirectoryInExePath=1`) goes to what `run_cmd`, `command`, `configured` and `shell::command` start, language servers and every terminal but an interactive shell's. A lookup that misses then tries the folders only a new sign-in's `PATH` has (`env::path_added`), so a program installed after the server started is found. What `command`, `configured` and `shell::command` start and language servers get those folders after the server's own `PATH` (`program_env`: `child_env` and `env::child_path`, before the caller's variables), so such a program finds what it runs in turn (gopls its `go`). git, started by name through std, keeps the server's own `PATH`, and its message says to restart (`INSTALLED_SINCE`). |
| `env` | `user_default()` (the environment a new sign-in of the user gets), `fresh_path()` (a terminal's `PATH`), `child_path()` (the `PATH` of a program the server starts by itself, `exe::program_env`) | `CreateEnvironmentBlock` for the process's token without its own variables, re-read once HKLM's or HKCU's `Environment` key changes (`RegNotifyChangeKeyValue`); `fresh_path` is its `Path`, then the process's own absolute entries it lacks; `child_path` is the process's own `PATH`, then that `Path`'s absolute entries it lacks (`None` when there are none). `None` on Unix: terminals, the programs the server starts and the service keep the process's environment. |
| `path` | `is_absolute_str`, `check_component` / `check_relative`, `stays_inside`, `to_slash`, `canonicalize`, `leaves_machine` / `leaves_machine_below` / `ancestors_leave`, `is_refused_link`, `strip_prefix`, file-URI helpers, `data_home`, `private_dirs`, `pgpass_file` | Drive letters and `\`; device names, `:` streams, 8.3 names and trailing dots refused; UNC roots unsupported; links followed one at a time and never to a network path or a device; dunce and an uppercase drive letter; case-insensitive comparisons (see "Paths" in the security model). Data in `%LOCALAPPDATA%`, config in `%APPDATA%`. |
| `net` | `interfaces`, `bind` (the server's socket), `kill_port_holders` | `GetAdaptersAddresses`; `[::]` made dual-stack; port owners from `GetExtendedTcpTable`, only the same user's processes. |
| `desktop` | `open_url`, `notify_send` | A Chromium browser from App Paths with `--app=`, else `ShellExecuteW`, for http(s) URLs only; no desktop notifications yet. |
| `session` | a terminal's processes: `register` (its `Handle` keeps the sid naming that session while a clone lives: the lingering processes follow it), `wait` (the leader's exit: portable-pty's code and signal, and `terminated` when a hang-up, terminate, kill or interrupt signal ended it, whatever code portable-pty adds; `ExitInfo::terminated`, with Workbench's own kills), `leader_exited`, `members`, `kill(sid, grace, leader_gone)` (returns once the session is empty and `leader_gone()` holds, or right after the forced end), `holders` (who holds a file open), `held_outside`, `runs_outside`, the PTY's `Output` (the redaction hold-back), and `PARENT_TERMINAL_VARS` (this OS's variables of the terminal Workbench was started from, cleared in its terminals) | Windows Terminal's `WT_SESSION` and `WT_PROFILE_ID` are cleared; a Job Object per terminal (`ProcGroup::attach_terminal`: what it starts may leave on request, `JOB_OBJECT_LIMIT_BREAKAWAY_OK`) in a pseudoconsole; hang-up is `ClosePseudoConsole`, then `TerminateJobObject`; the reader gets EOF once the job is empty; `wait` never says `terminated` (Task Manager's End task leaves exit code 1, like a program that exits with 1), so only Workbench's own kills count; the session stays registered, its job holding the leader's pid, until that and every `Handle` are over (Windows reuses pids at once); `RmGetList` for open files; sysinfo for cwd and command lines. |
| `watch` | `debouncer` (the files, config and Workspace watchers), `RECURSIVE`, `FOLDERS_MODIFY`, `RESCAN_IS_OVERFLOW` | Our own `ReadDirectoryChangesW` watcher, one recursive watch per root, overflow as `Flag::Rescan`, no file-id cache. |
| `helper` | `askpass_env` (the environment that makes git ask Workbench for credentials: `GIT_ASKPASS`, a `#!/bin/sh` wrapper on Unix), `askpass_prompt` (main.rs, before clap) | No script: `GIT_ASKPASS` and ssh's `SSH_ASKPASS` name `workbench.exe`, with `WORKBENCH_HELPER=askpass`, `SSH_ASKPASS_REQUIRE=force` and `GCM_INTERACTIVE=never`. |
| `support` | `Feature`, `unsupported`, `require`, `require_root`, `require_local_root` (a root the user names, links on the way included): what this OS leaves out (`/api/health`, `unsupported_platform`) | Dev containers, desktop notifications, gdb attach, rust-gdb's printers and network or WSL roots are unsupported; Services is experimental. Everything is supported on Linux. |
| `autostart` (Windows only) | the `Run` value, `StartupApproved`, Start Menu shortcuts, `start_detached` / `start_apart` (in the caller's environment or a given one), `elevated`, `interactive`, `message_box` | `workbench service` on Windows (`platform/service_windows.rs`, "Service install"). |
| `dll` | `restrict_search()`, called at the start of `serve` | `SetDefaultDllDirectories`: a DLL loaded by name (portable-pty's `conpty.dll`) comes only from the executable's folder or System32, never the current directory or `PATH`. A no-op on Unix. |

Windows builds use the MSVC target with a static C runtime (`server/.cargo/config.toml`). The
release archive adds `conpty.dll` and `OpenConsole.exe` from Microsoft's ConPTY package next to
`workbench.exe`, where portable-pty loads them instead of the console host built into Windows
(`os::dll` keeps it from finding a `conpty.dll` anywhere else).

### TypeScript

- A `FeatureModule` (`web/src/shell/types.ts`) has these parts:
  - `panels`: center-area kinds;
  - `toolWindows`: side tool windows;
  - `commands`: the palette, with an optional `shortcut` (and `global: true` for the rare shortcut that must also fire inside terminals and editors);
  - `topbar` and `statusbar` widgets;
  - `mobileTabs`, each with an optional `when(project)` (like tool windows: the phone shows only the tabs that apply, and a saved tab that does not apply falls back to the first one) and an optional `openPanel(panel) → boolean`: on a phone, `openPanel` selects the first visible tab that takes the panel (agents takes `terminal` and `agents.home`); panels no tab takes are not opened there;
  - `providers`: global components (listeners, dialog hosts). They wrap the app (`App.tsx` nests them around the shell), so each must render its `children`.
  - `searchProviders`: tabs of **Search Everywhere** (double Shift, `shell/SearchEverywhere.tsx`), each `{id, title, order, minQuery?, inAll?, when?, search(query, ctx, signal) → SearchItem[], hint?}`. Files (10) and Text (40, Find in Files) come from files, Symbols (20, `workspace/symbol` of running language servers; it never enables code intelligence) from lsp, and Actions (30, the palette's commands) from the shell. "All" shows the best few of each; Tab / Shift+Tab switch tabs, Enter opens, Shift+Enter opens to the side. Double Shift is two quick taps of Shift alone, watched in the capture phase without consuming anything, so it works in terminals and editors too.
- Panels, tool windows and mobile tabs may be `React.lazy` components: the dock, `ToolWindowArea` and the phone shell wrap them in `Suspense`. Both shells are lazy chunks, and so are `AnsiLog`, `Markdown` and Monaco.
- **Keyboard shortcuts** (`shell/CommandPalette.tsx`, `shell/paletteSearch.ts`) run in the bubble phase: a terminal keeps every key typed into it (Ctrl+T, Ctrl+K, Ctrl+P mean something to bash and Claude Code, like CLion's "Override IDE shortcuts"), and keys a focused widget handled (Monaco's own bindings) are not taken. Ctrl+K opens the palette elsewhere; Ctrl+Shift+P opens it from anywhere. Editor keys are Monaco *actions* added per editor (never `addCommand`, whose keybinding is global); features that hook every editor add theirs a microtask after `onDidCreateEditor` (see "Editor models"). The only capture-phase keys are the debugger's F7/F8/F9/Ctrl+F2 while the current project has a live session; a widget that handles one of them itself declares it with `data-wb-keys="F7 …"` on an ancestor (the git diff viewer's F7 = Next Difference), and terminals keep every key. The full list, with who owns each key, is under "Keyboard shortcuts" below.
- Shell actions (`web/src/shell/actions.ts`): `openPanel`, `closePanel`, `focusPanel`, `showToolWindow`, `toast` (options `actions: ToastAction[]` for several buttons and `code` for a monospace block, e.g. a permission request's command), `toastError`, `confirmDialog` (supports `typed` confirmation), `promptDialog`, `openSettings(section?)`, `addProjectInteractive`.
- `ErrorBox` shows `not_configured` errors as setup help with an "Open Settings" button (`settingsSection`, default `integrations`; not on a phone), and `unsupported_platform` errors in the same box ("Not available on Windows", the reason, no Settings or Retry).
- `api/health.ts` loads `GET /api/health` once after sign-in. `useUnsupported(feature)` / `unsupportedReason(feature)` and `useExperimental(feature)` read it; features hide what the server's OS leaves out (the dev container chip, status item, commands and the Services link to its panel), explain what is limited (the Attach to Process picker's gdb note, Settings › Notifications), and mark the Services tool window "experimental". Before the report arrives nothing is hidden. Setup hints that must name a place or a command where the server gives none take it from the report's `os` too (`configFileHint(os)` for config.toml, as Secrets names its fix and the port confirmation how a port is freed).
- `askAgent({projectId, prompt, …})` lives in `shell/agentBridge.ts` and calls `POST /api/agents/ask`.
- `devcontainerAction(projectId, 'panel' | 'start' | 'stop' | 'rebuild' | 'shell')` lives in `shell/devcontainerBridge.ts`. The devcontainer feature registers the handler, so a Start from the Apps tool window or the phone goes through its confirmation.
- Data comes from `api` (`api/client.ts`; `api.upload(path, blob, query, onProgress, signal)` streams a raw-body upload with progress; `getDeviceKey()` hands the device key to the service worker) and `useEvent` / `useInvalidateOn` (`api/events.ts`). After a reconnect or `lagged`, `installResync` (mounted once in `App.tsx`) refetches every active query once; `useInvalidateOn` only handles its own event types.
- Shared queries: `useProjects`, `useProject` and `useTerminals`; `installProjectsSync` (once) keeps the first two fresh from `projects.changed` and `git.changed`. The terminals slice keeps the `['terminals']` cache fresh.
- The theme preference is applied to `<html data-theme>` synchronously by the store (`state/store.ts`), before React re-renders, so xterm and Monaco effects read the new CSS variables. Highlighted code outside Monaco uses the `--syn-*` tokens.
- **Editor models** (files contract, `features/files/modelAccess.ts`): a project file's Monaco model is `file:///<projectId>/<path>` (`~abs` for absolute paths: `file:///~abs/etc/hosts`, and on a Windows server `file:///~abs/C%3A%5Cx` for `C:\x`, which `parseModelUri` gives back as `C:\x`), created only by the files slice's buffers. Other features use `modelUriString` / `parseModelUri` / `modelFile(uri, projectOnly?)` (the file an editor shows; lsp, debug and git all parse model URIs with it), `peekModel`, `readText`, `ensureModel` (+ `release`), `saveModel`, `isDirty`, `onBuffersChange`, `onBufferRevision`, and for the paths models carry `isAbsolutePath` (`/…`, and `C:\…` or `C:/…` on a Windows server, from `api/health.ts`), `basename` (also at `\` on Windows) and `samePath` (on Windows without regard to `/` versus `\` or ASCII case). Project-relative paths use `/` on every OS; Copy Path and drag and drop join them to the root with its own separator (`joinAbsolute`). Other schemes: `lsp-src://<pid>/<abs>` (lsp's read-only library files), `inmemory://debug-source/<sid>/…` (debug), `inmemory://git-diff/…` (git's diff sides). Features hooking every editor (lsp, debug, git) guard against hooking one twice and add their actions a microtask after `onDidCreateEditor`.
- **The editor gutter is shared.** The glyph margin (on in the files slice's editor) carries debug breakpoints and execution point, and Monaco's code-action lightbulb when a line leaves it no room (lsp quick fixes): debug ignores clicks on the lightbulb. VCS change bars are line decorations (files), blame is in the line numbers, diagnostics are markers (lsp), git's line checkboxes live in the glyph margins of its own diff editors (`inmemory:` models, which debug does not decorate). Context menu groups: `navigation` (Ask Agent), `1_lsp`, Monaco's `1_modification`, `9_cutcopypaste`, `9_git`, `y_debug`, `z_workbench`.
- **UI:** use `@/ui` components and the design tokens. Features must not introduce new colour literals. Monaco comes through `MonacoEditor` / `MonacoDiffEditor`, logs through `AnsiLog`, markdown through `Markdown`.

### Keyboard shortcuts

CLion's keymap where CLion has the action. *Palette* shortcuts are commands (`shortcut` in a `FeatureModule`), taken in the bubble phase outside terminals; *editor* ones are Monaco actions of the focused editor; *capture* ones are taken before any widget. Phase 3 added the ones marked ³; none collides with another: the pairs that share a key differ by modifier or focus as noted.

| key | action | owner, kind |
|---|---|---|
| Ctrl+K / Ctrl+Shift+P | command palette | shell |
| Ctrl+P | Go to File… | files, palette |
| Ctrl+Shift+F | Find in Files… | files, palette |
| Alt+F1 | Reveal Active File in Project Tree | files, palette |
| Alt+Shift+C ³ | Recent Changes (Local History) | files, palette |
| Ctrl+S | Save | files, editor (also the Local History diff) |
| Ctrl+Alt+Shift+Insert | New Scratch File… | files, palette |
| Ctrl+Alt+Z ³ | Revert Selected Lines (Local History diff) | files, editor |
| Ctrl+Shift+A | Ask Agent About Selection (editor) / New agent session (palette) | files editor, agents palette |
| Ctrl+, | Settings | platform, palette |
| F1 | Help (the user documentation) | help, palette |
| Alt+0 / Alt+9 | Commit… / Show Git Log | git, palette |
| Ctrl+T / Ctrl+Shift+K | Update Project… / Push… | git, palette |
| F7 / Shift+F7, Alt+↓ / Alt+↑ | Next / Previous Difference (diff viewer; declared with `data-wb-keys`, so it wins over Step Into) | git, diff panel |
| Ctrl+B / Ctrl+click ³ | Go to Declaration or Usages | lsp, editor |
| Ctrl+Alt+B ³ | Go to Implementation(s) | lsp, editor |
| Ctrl+Shift+B ³ | Go to Type Declaration | lsp, editor |
| Alt+F7 / Ctrl+Alt+F7 ³ | Find Usages / Show Usages (debug's F7 leaves Alt and Ctrl alone) | lsp, editor |
| Shift+F6 ³ | Rename… | lsp, editor |
| Ctrl+F12 ³ | File Structure (replaces Monaco's Go to Implementations there) | lsp, editor |
| Ctrl+Alt+L ³ | Reformat Code | lsp, editor |
| Alt+Enter ³ | Show Context Actions | lsp, editor |
| F2 / Shift+F2 ³ | Next / Previous Highlighted Error (in the editor; F2 in the Files tree renames) | lsp, editor |
| Ctrl+Alt+Shift+N ³ | Go to Symbol… | lsp, palette |
| Alt+6 ³ | Show Problems | lsp, palette |
| Shift+F9 / Alt+Shift+F9 ³ | Debug / Debug… (picker) | debug, palette |
| Ctrl+Alt+F5 ³ | Attach to Process… | debug, palette |
| F9 ³ | Resume Program (while a session of the project is live) | debug, capture |
| F8 / Shift+F8 ³ | Step Over / Step Out (live session; Monaco's F8 = next problem otherwise) | debug, capture |
| F7 ³ | Step Into (live session; not in a widget declaring F7) | debug, capture |
| Ctrl+F2 ³ | Stop Debugging (live session) | debug, capture |
| Ctrl+F8 ³ | Toggle Line Breakpoint | debug, editor (palette: the last focused editor) |
| Alt+F9 ³ | Run to Cursor | debug, editor |
| Ctrl+Shift+F8 ³ | Edit Breakpoint / View Breakpoints | debug, editor and palette |
| Alt+5 ³ | Show Debug | debug, palette |
| Ctrl+Alt+C ³ | Comment on the selected passage (Confluence page view, not while typing) | atlassian, page |
| Ctrl+K ³ | Insert link (Confluence rich editor; the editor keeps the key, the palette does not open) | atlassian, rich editor |
| Ctrl+Enter | submit (agent composer, comment and review boxes) | several, field |

### Panels

Panel ids must be stable so reopening focuses the existing panel. Params must be JSON-serializable because the layout is persisted.

| kind | params | owner | id convention |
|---|---|---|---|
| `agents.home` | `{}` | terminals | `agents.home` |
| `terminal` | `{terminalId}` | terminals | `terminal:<terminalId>` |
| `editor` | `{projectId, path, line?, column?, endColumn?, t?, mode?}`; `path` is project-relative, or absolute when `projectId` is null; `t` forces a re-navigation; `mode` (`'read'\|'split'\|'edit'`, Markdown only) shows the rendered page, both, or the source; unset follows the `markdownMode` preference (Read by default), and a navigation to a `line` shows the source | files | `editor:<projectId>:<path>` (`…#side` for a second copy) |
| `markdown` | `{projectId, path}`: a live preview that follows the disk (agents' `ui.open`); files open as pages in `editor` | files | `markdown:<projectId>:<path>` (`…#side` for a second copy) |
| `search` | `{projectId, query?}` | files | `search:<projectId>` |
| `diff` | `{projectId, path, mode: 'working'\|'staged'\|'commit'\|'compare', sha?, base?, head?}` | git | `diff:<projectId>:<mode>:<sha\|base..head\|''>:<path>` |
| `commit` | `{projectId, sha}` | git | `commit:<projectId>:<sha>` |
| `gitlog` | `{projectId, path?, ref?, lines?: 'a,b', worktreeLines?}` (`lines`: `git log -L`, "Show History for Selection") | git | `gitlog:<projectId>` |
| `conflict` | `{projectId, path}` | git | `conflict:<projectId>:<path>` |
| `mr` | `{projectId, iid}` | gitlab | `mr:<projectId>:<iid>` |
| `pipeline` | `{projectId, pipelineId}` | gitlab | `pipeline:<projectId>:<id>` |
| `job` | `{projectId, jobId}` | gitlab | `job:<projectId>:<id>` |
| `gitlab.issue` | `{projectId, iid}` | gitlab | `gitlab.issue:<projectId>:<iid>` |
| `confluence` | `{pageId, mode?: 'view'\|'edit'}` | atlassian | `confluence:<pageId>` |
| `jira` | `{key}` | atlassian | `jira:<key>` |
| `jira.board` | `{boardId}` | atlassian | `jira.board:<boardId>` |
| `app` | `{projectId, env?, run?, url}` | apps | `app:<projectId>:<env\|run>` |
| `settings` | `{section?}` | platform | `settings` |
| `help` | `{page?}` (a page slug: `getting-started`, `projects`, `agents`, `version-control`, `remote-access`, `service`, `configuration`) | help | `help` |
| `pr` | `{projectId, number}` | github | `pr:<projectId>:<number>` |
| `gh.run` | `{projectId, runId}` | github | `gh.run:<projectId>:<runId>` |
| `gh.job` | `{projectId, jobId}` | github | `gh.job:<projectId>:<jobId>` |
| `gh.issue` | `{projectId, number}` | github | `gh.issue:<projectId>:<number>` |
| `workspace.home` | `{scope?: projectId \| 'home', view?: 'trash'}` | workspace | `workspace.home` |
| `card` | `{scope, cardId, step?}` | workspace | `card:<scope>:<cardId>` |
| `devcontainer` | `{projectId}` | devcontainer | `devcontainer:<projectId>` |
| `db.console` | `{projectId, source, consoleId, request?: {sql, t}}` (a SQL console; `request` adds and runs a statement, Open Table) | db | `db.console:<projectId>:<source>:<consoleId>` (`main` is the source's default console) |
| `localHistory` | `{projectId, path, dir?, id?}` (a file's, a folder's or, with `path: ''`, the project's Recent Changes) | files | `localHistory:<projectId>:<path>` (folders end in `/`) |
| `lsp.source` | `{projectId, uri, line?, column?, endColumn?, t?}`: a read-only library file a language server pointed to (`lsp-src:` URI) | lsp | `lsp.source:<projectId>:<path>` |
| `debug.source` | `{projectId, sessionId, path \| sourceReference, name?, line?, column?, t?}`: a frame's source outside the project, or source only the debugger has | debug | `debug.source:<projectId>:<path>` or `debug.source:<projectId>:<sessionId>:ref<n>` |

### Tool windows

The IDs below are the defaults. The left stripe also carries the bottom-side icons, CLion-style.

- **left:** `files` (the project, or the scratch files: see "Scratch files" under Configuration), `commit` (Changes, with Changes / Stash / Shelf tabs), `agents`, `search`, `workspace` (order 35).
- **right:** `gitlab` (when `project.gitlab`), `github` (order 12, when `project.github`), `confluence`, `jira` (only when the project or site has Jira; Issues and Boards tabs), `apps`, `database` (db, 60: see "Database").
- **bottom:** `terminal` (shells/runs, order 10), `problems` (lsp: diagnostics, 15, Alt+6), `todo` (files: TODO / FIXME / XXX / HACK comments of the project or the current file, 16; `GET /api/projects/{pid}/files/todos` reuses Find in Files' walker and keeps matches inside a comment of the file's language; rescans 1.5 s after `fs.changed`), `usages` (lsp: Find Usages results, 17), `gitlog` (20, Alt+9), `debug` (25, Alt+5), `run` (run-configuration output, 30), `services` (devcontainer: this computer's Docker containers, compose projects and images, 35, Alt+8; see "Services (Docker)" under Dev containers), `activity` (platform: MCP calls and notifications, 40). Stripe icons are distinct: Find (left) is the only magnifier; Find Usages has a crosshair.
- **CLion keys.** Tool windows: Alt+1 Files, Alt+2 Bookmarks, Alt+3 Find, Alt+4 Run, Alt+5 Debug, Alt+6 Problems, Alt+7 Structure (lsp: the focused file's symbols in source order, the caret's one marked), Alt+8 Services, Alt+9 Git Log, Alt+0 Commit, Alt+F12 Terminal. Editor keymap (`lib/editorKeymap.ts`, Settings › General, per browser): CLion by default — Ctrl+D duplicate, Ctrl+Y delete line, Ctrl+Shift+Z redo, Alt+J / Alt+Shift+J / Ctrl+Alt+Shift+J occurrences, Ctrl+Shift+J join, Ctrl+Shift+Up/Down and Alt+Shift+Up/Down move lines, Shift+Enter / Ctrl+Alt+Enter new line after / before, Ctrl+Q quick docs, Ctrl+Shift+/ block comment, Ctrl+Alt+O optimize imports, Ctrl+Alt+I auto-indent, Ctrl+Shift+M matching brace, Ctrl+Shift+U toggle case, Ctrl+NumPad+/- folding — as keybinding rules and one global editor action, swapped live; "VS Code" keeps Monaco's own keys. Ctrl+W (Extend Selection) cannot be taken from a browser.
- **Navigation (files):** `navHistory.ts` keeps where the caret has been in the active editor (one entry per place: moves within 10 lines update it, a file switch or a longer jump adds one; Back / Forward update the entry they go to instead of adding). Navigate Back / Forward: Ctrl+Alt+Left / Right (also as editor actions), Alt+Left / Right outside terminals, and the mouse's back / forward buttons (which would otherwise leave Workbench). Recent Files (Ctrl+E; again: changed files only, from the shared git status query) preselects the previous file; Recent Locations (Ctrl+Shift+E) lists the places with their code. Kept in memory per browser tab.
- **HTTP Client (apps):** JetBrains `.http` / `.rest` files. `server/src/apps/http_client.rs` parses them (`###` blocks, `# @name`, `@var = value`, `METHOD URL [HTTP/x]`, `?`/`&` continuation lines, headers, a body or `< ./file`; response-handler scripts and `>>` redirects are skipped, never run), fills `{{variables}}` from the file, `http-client.env.json` and `http-client.private.env.json` (nearest ones up to the project root; `$shared` plus the chosen environment; the private file is a default-sensitive file) and dynamic ones (`$uuid`, `$timestamp`, `$isoTimestamp`, `$randomInt`), and sends one request on the user's click (`POST /api/projects/{pid}/http/run {path, line, env?}`; devices only, agent tokens refused; http/https only; 60 s, 10 redirects, 5 MB of body). Values from the private file are masked (`••••`) in the request echoed back. `GET …/http/envs?path=` lists environments and requests. Editor (`features/apps/http`): a Monarch grammar (`http` language), CodeLenses "▶ Send Request" and "Environment: …" above every request, Ctrl+Enter at the caret; unsaved files are saved first. Responses go to the `httpResponse` panel (`{projectId}`, id `http:<projectId>`): status, time, size, Body (JSON pretty-printed), Headers, Request, Send Again, and the last 30 runs.
- **CI test reports and artifacts (gitlab, github):** a GitLab pipeline with a JUnit report gets a Tests tab in its `pipeline` panel (the header's "N failed" opens it): `GET /api/projects/{pid}/gitlab/pipelines/{id}/tests` keeps the report's counts and only its failed and errored cases (at most 200; output = failure message then stack trace, 12 KB each; `recentFailures` on the base branch, shown as "3× on main"), each with Ask agent to fix (the test, its file and output; `testFixPrompt`), Open the file and Copy. Agents read the same through `gitlab_test_failures`. GitLab job artifacts download from the job panel (`jobs/{id}/artifacts`, streamed); GitHub run artifacts are listed in the `gh.run` panel (see the GitHub REST list).
- **Compare (files):** panel `compare` `{projectId, path, left: {path} | {textKey, label}}` — a project file (right, its buffer, editable) against another file of the project (left, its buffer, editable) or the clipboard (read-only; the text stays in the browser tab, so a reloaded layout says it is gone). Compare With… (tree, editor, palette; the other file comes from Go to File in pick mode, `useQuickOpen.choose`), Compare with Clipboard (editor context menu, palette); Ctrl+S saves the side it is pressed in; Swap sides. Alt+Shift+Insert toggles Column Selection Mode.
- **Bookmarks (files):** `bookmarks.ts` (a zustand store persisted per browser, every project). F11 toggles one on the caret line, Ctrl+F11 with a mnemonic (0–9, A–Z; unique, taking one moves it), Shift+F11 lists them (typing a mnemonic into the empty list jumps; Delete removes). The editor shows them in the glyph margin's right lane (beside breakpoints) with a decoration collection per editor that follows edits and writes the new lines back.
- The palette generates a "Show <title>" command per tool window the current project shows (its `when`) unless a feature command already has that title ("Show Git Log", Alt+9).
- **Phone tabs:** `agents` (permission requests answerable in rows and the full-screen terminal), `git` (with the bisect banner), `workspace` (25), `files`, `ci` (GitLab, when `project.gitlab`), `github` (42, when `project.github`), `docs` (when Confluence is set up), `apps`, `more` (a Help button first, then Notifications: push on/off, topics, test). Help on a phone lives in the More tab: the page list with search, a page with an "All pages" button (`features/help/mobile.ts` holds its state; the More tab's `openPanel` takes the `help` panel). Code intelligence, the debugger and Local History are desktop-only.

### Events

Wire format: `{type, projectId?, data, ts}` on `/api/events/ws`.

The client sends `{"type":"ping"}` every 25 s and gets `pong`. When the device's session ends the server closes the socket with code `4401`.

| type | data | emitted by |
|---|---|---|
| `hello`, `pong`, `lagged` → client `resync` | | core |
| `projects.changed` | `{}` | core |
| `settings.changed` | `{}` (config.toml was saved, or edited outside Workbench and applied) | platform |
| `ui.open` | `{panel, params, title?, id?}`; `id` is the stable panel id (`events.ui_open_id`) | core helper; files, git, gitlab and platform MCP tools |
| `ui.notify` | `{level, message}` | anyone |
| `terminal.created` / `terminal.updated` / `terminal.exited` / `terminal.removed` | `TerminalInfo` (removed: `{id}`) | terminals |
| `agent.attention` | `{terminalId, state, message, title, permission}`; `permission`: the `PendingPermission` Workbench can answer, or null (each answerable request gets its own event) | terminals |
| `fs.changed` | `{paths: string[], overflow?}` (project-relative, `/`-separated on every OS; `overflow`: too many to list, or the watcher lost events, refresh everything) | files |
| `files.history` | `{paths}` (Local History recorded versions or labels of these paths) | files |
| `git.changed` | `{}` (HEAD, index or refs moved) | files watcher / git ops |
| `git.op` | `{opId, op, title?, line?, done?, ok?, message?}` | git |
| `git.commitMessage` | `{projectId, message, terminalId}` (an agent proposed a commit message) | git (MCP) |
| `git.changelists` / `git.shelf` | `{}` (changelists or shelves changed) | git |
| `lsp.state` | `{server, state, progress, transition}`, or `{enabled}` / `{settings}` for the project | lsp |
| `lsp.diagnostics` | `{errors, warnings, infos, hints, files}` (debounced counts) | lsp |
| `debug.session` | `SessionInfo`; `{id, projectId, removed: true}` when an ended session is forgotten | debug |
| `debug.output` | `{sessionId, lines}` (a flood sends 500 lines; the UI fetches the rest) | debug |
| `debug.breakpoints` | the whole breakpoints view (lines, functions, exception filters, muted, watches) | debug |
| `run.state` | `{name, state, port?, url?, terminalId?, startedAt?, readyAt?, exit?, result?, error?, terminated?, phase?, inContainer?, reach?}` | apps |
| `devcontainer.state` | `{projectId, state, containerId?, inContainer}` (`none`, `stopped`, `running`, `building`, `error`) | devcontainer |
| `docker.changed` | `{kinds: ('container' \| 'image')[]}` (a container or image changed; `docker events`, 300 ms debounce, `exec_*` ignored; also after each Services action) | devcontainer |
| `env.health` | `{env, status, httpStatus?, latencyMs?, version?, checkedAt, error?, sample?}`; partial updates `{env, previewChanged}`, `{env, deploying}`, `{env, versionInfo}` | apps |
| `gitlab.pipeline` | `{pipelineId, iid?, status, ref, sha, webUrl, previousStatus?}` | gitlab |
| `gitlab.job` | `{jobId, previousJobId, action, status, pipelineId?}` | gitlab |
| `gitlab.mr` / `gitlab.issue` | `{iid, action}` | gitlab |
| `confluence.page` | `{pageId, version?, title?, action}`; actions also `comment-updated`, `comment-resolved`, `comment-reopened`, `comment-deleted`, `attachment-added`, `attachment-deleted`, `labels`, `moved`, `deleted`, `restored` | atlassian |
| `jira.issue` | `{key, action}` | atlassian |
| `github.run` | `{runId, workflowId, name, state, status, conclusion, branch, sha, event, runNumber, webUrl, action, previousState?}` (a dispatch: `{runId: null, workflowId, action: 'dispatch', branch}`) | github |
| `github.job` / `github.pr` / `github.issue` | `{jobId, runId, action}` / `{number, action}` / `{number, action}` | github |
| `workspace.changed` | `{scope, cardId?}` (`projectId` = the scope's project) | workspace |
| `workspace.trash` | `{scope}` (a card went to the trash, was restored or purged) | workspace |
| `push.changed` | `{}` (a device's push subscription was added, changed or removed) | platform |
| `mcp.call` | one activity record (tool, ok, ms, terminalId…) | platform |
| `platform.activity` | one activity record (kinds `attention`, `env`, `deploy`, `pipeline` for GitLab pipelines and GitHub workflow runs, `notify`) | platform |

**Files watcher** (`files/watch.rs` over `util::os::watch`): one per project, 200 ms debounce, 500 paths per event. Linux: an inotify watch per directory the tree shows (gitignore-aware, at most 8000), added as folders appear. Windows: one recursive `ReadDirectoryChangesW` watch on the root, since an open directory handle keeps the folders above it from being renamed; only changes in the folders the same walk covers are kept (not in hard-ignored or gitignored ones), and a folder Windows reports as modified because its entries changed is dropped, so both report the same paths. On Windows a lost batch of notifications (the 64 KB buffer overflowed) is `overflow: true`, and a watch that stops on an error is made again (after 1 s, doubling), also with `overflow: true`; inotify's queue overflow is not reported. After lost notifications Local History gets the paths the batch did report plus the files the same walk finds modified since 5 s before the previous batch was taken (`changed_since`, at most 50,000 entries looked at; more than 500 such files are a checkout and left to the VCS, as a batch over the cap is). That walk runs in a task of its own, one at a time (lost changes that come during a walk are merged for the next), so `fs.changed` and `git.changed` never wait for it. `GET …/files/watch` reports `dirs` (the folders covered), `capped` and `errors`.

**Terminal socket** (`/api/terminals/{id}/ws`): the server sends `{t:"snapshot", cols, rows}` followed by a binary snapshot, then binary output; `{t:"resync", cols, rows}` + a binary snapshot when the client fell behind; `{t:"exit", code, signal}` and `{t:"running"}`. The client sends binary input, `{t:"resize", cols, rows}` and `{t:"ping"}` (answered with `{t:"pong"}`).

**Claude Code hooks:** hosted sessions report state through HTTP hooks on `/api/hooks/**`, except `SessionStart`, which Claude Code runs only as a command hook: the `workbench statusline` helper posts it.

### REST prefixes

| prefix | owner |
|---|---|
| `/api/health`, `/api/auth/**`, `/api/projects` (list/add/reload/detail/delete), `/api/events/ws` | core |
| `/api/terminals/**`, `/api/agents/**` (incl. `POST /api/agents/{id}/permission`, devices only), `/api/hooks/**` | terminals |
| `/api/projects/{pid}/files/**` (incl. `files/history/**`: Local History), `/api/projects/{pid}/search`, `/api/fs/**` | files |
| `/api/projects/{pid}/lsp/**` (incl. the `lsp/ws` editor socket) | lsp |
| `/api/projects/{pid}/debug/**` | debug |
| `/api/projects/{pid}/git/**` | git |
| `/api/projects/{pid}/gitlab/**`, `/api/gitlab/**` | gitlab |
| `/api/projects/{pid}/github/**`, `/api/github/**` | github |
| `/api/workspace/**` (incl. `{scope}/trash/**`), `/view/{grant}/**` (capability URLs, public path) | workspace |
| `/api/atlassian/**`, `/api/confluence/**`, `/api/jira/**` | atlassian |
| `/api/projects/{pid}/runs/**`, `/api/projects/{pid}/envs/**` | apps |
| `/api/projects/{pid}/devcontainer/**`, `/api/docker/**` (Services: containers, compose projects, images) | devcontainer |
| `/api/projects/{pid}/db/**` | db |
| `/mcp`, `/api/platform/**`, `/api/settings/**`, `/api/push/**` | platform |
| `/sw.js`, `/manifest.webmanifest`, `/icons/**` (public, served by `spa.rs` from `web/public`) | platform files, core serving |

`GET /api/health` (public) → `{ok, service, version, startedAt, os, unsupported, experimental}`: `os` is `linux`, `windows` or `macos`; `unsupported` maps a feature to why this OS leaves it out and `experimental` to a note (both empty on Linux). Features on Windows: `devcontainer`, `desktopNotifications`, `gdbAttach`, `rustGdbPrettyPrinters` and `networkRoots` are unsupported, `services` is experimental.

**App previews** are not proxied under `/api`: `GET /api/projects/{pid}/envs/{name}/proxy-url` starts a per-env proxy on its own loopback port (`apps/proxy.rs`) and returns a one-time URL. It is local-only (`url: null` for remote devices).

Cross-slice REST contract: `POST /api/agents/ask {projectId, prompt, terminalId?, newSession?, name?, submit?} → TerminalInfo`. It pastes the prompt into the target agent session: the given one (409 while it shows a dialog), or the project's most recently active one that can take it unasked (see "Agent providers"), or a new one.

### Cross-slice data contracts

These are consumed by a slice other than their owner, so their shapes are fixed. Owners may add fields but not rename or remove them.

```ts
// git — GET /api/projects/{pid}/git/status   (files tree colours, status bar, commit window)
//       shared react-query key ['git', pid, 'status']; the git slice refreshes it on git.changed / fs.changed / resync
interface GitStatus {
  branch: string | null            // null when detached
  head: string | null              // full sha, null before the first commit
  upstream: string | null
  ahead: number; behind: number
  state: 'clean' | 'merging' | 'rebasing' | 'cherry-picking' | 'reverting' | 'bisecting'
  stashes: number
  files: { path: string; origPath?: string
           index: ' '|'M'|'A'|'D'|'R'|'C'|'T'|'U'|'?'|'!'   // staged side
           worktree: ' '|'M'|'A'|'D'|'R'|'C'|'T'|'U'|'?'|'!' // unstaged side ('?' untracked)
           conflict: boolean }[]
}
// git — GET /api/projects/{pid}/git/diff?path=&mode=working|staged|commit|compare&sha=&base=&head=
interface GitFileDiff {
  path: string; oldPath?: string
  original: string; modified: string          // full texts ('' when absent); the working tree as git reads it
                                               // (LF where git turns its CRLFs into LFs, like the hunks)
  binary: boolean; tooLarge: boolean
  hunks: { header: string; oldStart: number; oldLines: number; newStart: number; newLines: number }[]
  fingerprint: string                          // pass back when staging hunks or lines
  canSelectLines: boolean                      // line staging / partial commit offered
  lines: { hunk: number; kind: 'add' | 'del'; line: number; at: number }[]
}
// git — GET /api/projects/{pid}/git/blame?path=&rev=      (time: Unix milliseconds)
interface GitBlame { lines: { line: number; sha: string; author: string; time: number; summary: string }[] }

// terminals — POST /api/terminals {kind: 'shell', projectId, cwd?: string (project-relative or absolute), cols?, rows?,
//             container?: boolean (true: in the project's dev container; false: on the host; absent: the project's default)} → TerminalInfo
// terminals — POST /api/agents/ask → see above; POST /api/agents {projectId, prompt?, name?, model?, effort?,
//             permissionMode?, remoteControl?, resume?: sessionId, fork?: boolean, inContainer?: boolean} → TerminalInfo
// core — ProjectSummary.devcontainer: {configs: string[], state: 'none'|'stopped'|'running'|'building'|'error', inContainer: boolean} | null
// terminals — AgentInfo.pendingPermission (platform push reads it; so do the toast, cards and phone rows)
interface PendingPermission {
  id: string; tool: string
  summary: string                   // one line, masked, cut at 140: never enough to approve on
  since: number                     // ms
  sessionRule?: string | null       // what "For session" allows, whole
  detail?: string                   // the whole request on its real lines (≤ 16 KB), masked only for secrets
  complete?: boolean                // detail and sessionRule are whole: one-tap Allow only then
}
// terminals — POST /api/agents/{terminalId}/permission {id, decision: 'allow'|'deny', message?, interrupt?,
//             scope?: 'once'|'session'} → TerminalInfo; devices only; 409 not_pending once settled
// terminals — TerminalInfo.meta.inContainer: true, meta.container: {id, name, user, folder, docker}: the process runs in the dev container
```

**Drag and drop.** Dragging a file or folder carries MIME `application/x-workbench-path` (the absolute path) plus `text/plain`. For example, drag from the files tree onto an agent terminal, and the path is inserted, quoted.

### MCP tools

Tool names are prefixed by domain: `workbench_` (the UI, sessions, and the files and git tools: `workbench_open_file`, `workbench_show_diff`, `workbench_set_commit_message`, `workbench_changelists`…; git tools take `project` and are confined to the session's project), `gitlab_`, `github_`, `workspace_`, `confluence_`, `jira_`, `env_`, `run_`, `devcontainer_` (`devcontainer_status`, read-only), `code_` (lsp), `debug_` (`debug_state`), `files_` (`files_local_history`). `GET /api/platform/tools` lists them. Each has a JSON Schema for its input. Set `mutating: true` when the tool changes remote state.

Added in the third phase (all confined with `McpCtx::project_for`):

| tool | owner | what |
|---|---|---|
| `code_diagnostics {path?}`, `code_symbols {query}`, `code_definition {path, line, column}`, `code_references {path, line, column}` | lsp | read-only; only servers that already run (a tool never enables or starts one) |
| `debug_state {sessionId?, frame?}` | debug | read-only: sessions, the stop, the stack, a frame's locals (secret values masked), the console's newest tail |
| `workbench_changelists` | git | read-only: changelists and shelves with their files |
| `files_local_history {path, limit?, revision?, diff?, against?, content?}` | files | read-only; sensitive paths refused |
| `confluence_add_inline_comment` (mutating), `confluence_upload_attachment` (mutating; a file of the session's project, never hidden, key/token-named or `sensitive` files, symlinks resolved first), `confluence_labels` (mutating) | atlassian | Confluence authoring |
| `jira_boards`, `jira_board_issues` | atlassian | read-only Jira Software boards |
| `gitlab_test_failures {pipelineId}` | gitlab | read-only: the failed and errored tests of a pipeline's JUnit report, with file and output |

Only expose what an agent cannot do easily with its own shell, or what needs Workbench's credentials or UI:
- driving the UI (open a file or diff, notify);
- Confluence, Jira, GitLab and GitHub;
- Workspace cards (deliverables the owner reviews);
- environment health;
- run-configuration output;
- what only Workbench's running services know: language servers' diagnostics and symbols, a debug session's state, Local History, changelists.

Agents never enable code intelligence, start, step or stop a debugger, answer a permission request, stage, commit, shelve, rebase or bisect through MCP.

**Never** expose deploys or destructive git operations to agents. This includes run configurations that deploy or release: `run_start` refuses runs with `needsConfirm` (and documentation suggestions), which the UI starts only after the user confirms. It also refuses every run to a session whose CLI confines its commands to a sandbox (Codex, unless it bypasses it: `Terminals::sandboxed_agent`): a run's command comes from files such an agent can edit (Makefile, package.json), and Workbench would run it outside the sandbox.

## Development

```bash
# backend (Rust 1.97, edition 2024)
cd server
export CARGO_TARGET_DIR=~/.cache/workbench-targets/<name>   # never /tmp (RAM tmpfs)
cargo test
cargo run -- serve --bind 127.0.0.1:7777
cargo run -- open                      # a browser window for the running server (one-time code, no token in argv)
cargo run -- service install --dry-run # what `workbench service install` would write

# frontend
cd web
npm ci
npm run build      # tsc -b && vite build — the real type check; the debug server serves web/dist live
npm run dev        # :5173 proxying /api, /auth, /pair, /mcp and /view to WORKBENCH_BACKEND (default http://127.0.0.1:7777)
npm run lint
npm test           # vitest (src/**/*.test.ts)
```

- **Isolated instance for development or tests.** Set `WORKBENCH_CONFIG_DIR` and `WORKBENCH_DATA_DIR` to scratch directories and choose a free `--bind` port.
- **Signing in.** Use `$(cat $WORKBENCH_DATA_DIR/token)` as the Bearer token for curl. Open `/auth?token=…` in a browser.
- **Stopping a server.** Use `fuser -k <port>/tcp`, never `pkill -f`.
- **CI** (`.github/workflows/ci.yml`, GitHub Actions): every push to `main` and every pull
  request runs the web job (`npm ci`, build, lint, test; Node 22) and the server job
  (`cargo build --locked`, `cargo test --locked`; stable Rust) on Ubuntu 24.04. The
  `windows-latest` job builds the server (`cargo build --locked`), runs `cargo test --locked
  --no-fail-fast` (with Python for the test fakes and `core.autocrlf false`; its Rust cache is
  kept when tests fail) and then, once the build has succeeded and whether the tests passed or
  not, `install.ps1` under Windows PowerShell 5.1: an install, then `-Uninstall`, refused
  (naming the server's pid) while the installed server runs and then checked to remove the
  service's Start Menu shortcut, the folder and exactly the PATH entry it added. It counts
  like the other two jobs: a failed step fails the run (no `continue-on-error`).
- **Releases** (`release.yml`): bump `version` in `server/Cargo.toml` (and `web/package.json`),
  give CHANGELOG.md a `## X.Y.Z - date` section, commit, then push a `vX.Y.Z` tag. The
  workflow refuses a tag that does not match the crate version, builds the UI and the
  release binary on Ubuntu 22.04 (glibc 2.35 is the floor; `cargo build` also makes the
  `workbenchw` stub, which the archive leaves out), strips the binary, starts it on a
  scratch config to check that the embedded UI is served, and publishes
  `workbench-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` (binary, `install.sh`, LICENSE, README,
  CHANGELOG, notices) with a `.sha256`, the CHANGELOG section as the notes. Started by hand,
  it builds the archives as artifacts without publishing.
- **The Windows release** is a job of its own on `windows-latest`. It builds the UI,
  `workbench.exe` and `workbenchw.exe` (MSVC, static C runtime), takes `conpty.dll` and
  `OpenConsole.exe` (x64) from the pinned `Microsoft.Windows.Console.ConPTY` NuGet package
  (checked against pinned SHA-256s), installs the staged package with `install.ps1` under
  Windows PowerShell 5.1, checks that the binary imports no Visual C++ runtime, starts it on
  scratch directories and a free port until the UI is served, installs again over the
  running server (whose exe must end up renamed aside), and builds
  `workbench-X.Y.Z-x86_64-pc-windows-msvc.zip`
  (`workbench.exe`, `workbenchw.exe`, `install.ps1`, `conpty.dll`,
  `OpenConsole.exe`, LICENSE, README, CHANGELOG, the notices and `CONPTY_NOTICE.md`) with a
  `.sha256`. Started by hand, the job always runs; on a tag it runs only while the repository
  variable `RELEASE_WINDOWS` is `true`, and `publish` then needs both jobs. Until then a tag
  publishes the Linux archive alone, as before the port. `install.ps1` installs per user into
  `%LOCALAPPDATA%\Programs\Workbench` (or `-Prefix`) without elevation, gives a folder it
  creates an access list for the user, SYSTEM and Administrators only (and warns when an
  existing one lets others write), adds it to the user PATH (`HKCU\Environment`, then
  `WM_SETTINGCHANGE`), renames files in use aside (`*.old`, removed by the next install,
  renames retried on sharing violations), removes the Mark of the Web from what it installs
  and exits non-zero on failure. `install.ps1 -Uninstall` (same `-Prefix`) changes nothing
  while a program runs from the folder, or when it cannot tell (processes by executable
  path through `QueryFullProcessImageNameW`, no WMI; a `workbench.exe`, `workbenchw.exe` or
  `OpenConsole.exe` of the current session whose path cannot be read counts as running),
  runs `workbench service uninstall [--name N]` for the services whose `Run` value or Start
  Menu shortcut starts that folder's `workbenchw.exe` (another folder's stay), deletes the
  files `install.ps1` puts there, removes exactly the PATH entry it added (the value keeps
  its type; `WM_SETTINGCHANGE`), then deletes `workbench.exe` (so running it again finishes
  an interrupted run) and the folder when nothing else is left in it, leaves a folder
  without `workbench.exe` alone, and keeps the configuration and data folders, printing
  where they are.

## Second phase (2026-09-26): Workspace, agent providers, GitHub, broader detection

**Forge dispatch.** Code that only needs "the CI state of this commit" calls
`forge::commit_ci_status(state, project, sha)`. It routes to `gitlab::commit_ci_status`
or `github::commit_ci_status`, which returns the same `CiStatus` with GitLab's vocabulary.
`Project::github()` mirrors `Project::gitlab()`. `ProjectSummary.github` is `{host, path}` or null.
The global `[github]` config holds `host` and a `token` secret name; without a token only public
repositories work, unauthenticated. Repository layers follow the same trust rules as GitLab:
an empty `repo.github.token` means the global token, used only when the host matches.

**Workspace (deliverable cards).** Adapted from Mr. Mak Workspace (MIT) for any project. A scope is
a project id or `home` (not tied to a project). No project gets the id `home` or `all` (the home panel's
"All" view): the registry reserves them, so a directory called `home` is project `home-2`.
- **Where cards live.** `data_dir/workspace/<scope>/workspace.json` in Mr. Mak's schema
  (`{entities:[{id,title,description,icon?,type,category,created,updated?,folder,steps:[{name,path,viewer?}],
  defaultStep?,status,pinned?,sample?}]}`); files in `data_dir/workspace/<scope>/<YYYY-MM-DD_slug>/`, so
  repositories stay clean. Every scope gets `_shared/report.css` and `report.js` (dark report styles and
  an image lightbox) that reports link as `../_shared/…`.
- **Registry writes** are compare-and-swap through an order-preserving JSON tree, so unknown keys, key
  order and entries we cannot use survive: the new file is staged and synced first, then swapped in with
  `renameat2(RENAME_EXCHANGE)` only while the registry still holds what was read (checked again after the
  swap, which is undone on a mismatch); otherwise the change is redone on the newer file (409 after 8
  tries). Windows has no atomic exchange (`util::os::fs::rename_exchange` is unsupported there): the check
  right before a plain rename is the last one. A registry that does not parse is never overwritten. Our registries use UTC timestamps; `updated` is bumped on any change.
- **Lists** sort pinned first, then by freshness (`updated ?? created`, newest first); a card is
  `archived` when its status says so or it was not touched for 7 days (unless pinned or a sample).
- **Mr. Mak compatibility.** A project whose root has `workspace/workspace.json` also shows those cards,
  as origin `repo` with id `repo:<id>`. They are repository content: folders must be one plain path
  component, paths are contained, and only `status`, `pinned` and `updated` (a local day) are written
  back, with the same compare-and-swap. Steps, files and deletion stay with the repository.
- **Examples** (`examples.rs`; files in `examples/`, screenshots from `docs/assets`). At start,
  while `data_dir/workspace/home/workspace.json` does not exist, four `sample` cards go into Home
  (Welcome to Workbench, pinned; A tour of Workbench; Hand work to an agent; Connect your
  services), in folders `<day>_<id>`, one second apart in freshness so they list in that order.
  The files are written first and the registry last, only while it still has no entries (else the
  folders are removed again). Once Home has a registry, even an empty one, nothing is added, so an
  archived or deleted example stays that way.
- **REST** `/api/workspace/`: `GET scopes`; `GET cards` (every scope); `GET|POST {scope}/cards`;
  `GET|PATCH|DELETE {scope}/cards/{id}` (delete moves the folder to `data_dir/workspace-trash/`);
  `POST …/steps`, `PATCH|DELETE …/steps/{index}` (with the expected path: 409 when steps moved);
  `GET …/files?path=&offset=` (folders first, then natural name order; 2000 entries a page, with
  `truncated` and `total`); `GET|PUT …/content` (text up to 2 MB; markdown saves check a sha256
  revision, 409 on conflict, previous version kept in `data_dir/workspace-backups/`); `POST …/upload?name=&dir=&step=` (raw body,
  never replaces a file); `POST …/grant`. A step path is relative to the card folder, or absolute inside
  it or inside a project root (then copied in under a free name, never through an existing entry such
  as a symlink an agent planted in its card folder; the UI may use any project, an agent only its own).
  Viewers: `auto` (by extension; a folder is a gallery, a `.glb` opens in the 3D viewer), `html`,
  `markdown`, `image`, `gallery`, `compare3d` (Mr. Mak's manifest), `pdf`, `video`, `audio`, `text`.
- **Sandboxed content.** Card files are served only from capability URLs `/view/<grant>/<folder>/<path>`.
  A grant (random, in memory, 12 h, reused while it has an hour left) is minted by an authenticated call
  (`…/grant`, or embedded as `base` in every card with its `grantExpiresAt`; the UI refetches cards
  every 20 minutes and on focus, so what is on screen keeps a live grant) and covers one card folder plus
  the scope's `_shared/`. Every response, errors included, carries `Content-Security-Policy: sandbox
  allow-scripts allow-popups allow-downloads; default-src 'self' 'unsafe-inline' data: blob:`,
  `nosniff`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store` and `Access-Control-Allow-Origin: *`
  (the report's own opaque-origin fetches of its files; never credentials). Without `allow-same-origin`
  a report cannot read Workbench's cookies or storage, reach its parent, or read `/api` (verified in
  headless Chrome: the session cookie is not even sent). Dotfiles and credential names are refused,
  paths are resolved with symlink containment, ranges are served, and HTML gets Mr. Mak's prelude (dark
  scrollbars, external links) inserted after `<head>`, the doctype or the BOM. The UI frames reports with
  the same sandbox attribute. Popups a report opens stay sandboxed (no `allow-popups-to-escape-sandbox`:
  an escaped popup could run Workbench on its own origin with a URL the report chose, e.g. `/#wbk=…`).
  External http(s) links still open normally: in a frame the prelude posts `{type: 'workbench:open-link',
  href}` to the parent, which opens it with `noopener` (never Workbench's own origin or port); a report
  in a tab of its own follows the link in that tab. PDFs are fetched through the grant and shown from a
  blob URL (browsers refuse their PDF viewer in a sandboxed document).
- **Agents** (MCP, descriptions double as the authoring guide): `workspace_list_cards`,
  `workspace_create_card` (→ `{cardId, folder}`), `workspace_add_step`, `workspace_write_file` (for
  agents that cannot write outside their project), `workspace_update_card`, `workspace_open_card`
  (`ui.open` `card`). The default scope is the session's project, else `home`; a session may name only
  its own project or `home`.
- **Events.** `workspace.changed {scope, cardId?}` (`projectId` = the scope's project) after every change,
  and from a debounced watcher on `data_dir/workspace` and each project's `workspace/` directory
  (directories, not files, because atomic saves replace inodes), so agent and external edits show live.
  Reads (inotify open/access) are not changes. A repository's `workspace/` arriving whole (checkout,
  clone, `cp -r`; `fs.changed` names just `workspace`) starts its watch. Trash and backups live outside
  the watched tree.
- **UI.** `workspace.home` (grid: pinned section, category groups, search through the archive, archive
  toggle, scope switcher project / Home / All, New card), `card` (header with status, pin, ask agent,
  card folder drawer; one tab per step; drops of `application/x-workbench-path` or desktop files add
  steps), tool window `workspace` (left, order 35), palette commands (Workspace home, New card…, Open
  card…) and the phone tab `workspace` (order 25). Markdown drafts are kept per file (and in the tab's
  sessionStorage) across step switches, `ui.open` and reloads, with an unload warning; links out of a
  repository card open the project's file, others out of the card only toast. The 3D viewer is a lazy three.js chunk; it decodes
  Draco with the asm.js decoder because the SPA's CSP allows no WebAssembly (meshopt-compressed models
  are not supported; `vite.config.ts` drops DRACOLoader's default WebAssembly decoder URLs so they are not
  bundled), and decodes textures through `<img>` because ImageBitmapLoader's `fetch(blob:)` is refused by
  `connect-src 'self'`.

**GitHub.** `server/src/github/**` and `web/src/features/github/**`, modelled on the GitLab slice.
- **Connection** (`client.rs`). API base `https://api.github.com` for github.com, `https://<host>/api/v3`
  (GraphQL `/api/graphql`) for Enterprise; `host` may carry a scheme (`http://127.0.0.1:8931`) for
  Enterprise servers and mocks. `owner/repo` is checked to be two name segments before any URL is built.
  The token is `repo.github.token` (resolved by `AppState::secret`, so a name from repository config
  only resolves against the overlay), else `[github] token` when its host equals the project's
  **including the scheme** (an `http://github.com` repository host never gets it). No token means
  **anonymous public mode**: reads only (writes answer `not_configured` before sending anything),
  60 requests an hour per IP, and the UI shows a "Public, read-only" banner with the quota. A
  repository that answers 404 is remembered (10 min anonymous, 1 min with a token); anonymous, that is
  what a private repository looks like, so it is `not_configured` (setup help), not `not_found`.
- **Requests.** The token only goes into `Authorization` on the API origin. Redirects are followed by
  hand: same-origin with the token, cross-origin (signed job-log URLs) without it. GETs go through an
  ETag cache (`Fresh::Live` 3 s / `Slow` 60 s with a token, 5 min / 15 min anonymous, since an anonymous
  304 still costs a request; `Fixed` a day) and are conditional after that. Writes also drop the
  repository metadata (open issue counts); a run the poller saw change drops its cached detail, job
  details and its commit's checks. The summary is shared for 8 s (5 min anonymous) while the local
  branch and HEAD stay the same. `x-ratelimit-*` is remembered per API, identity and resource: while the
  quota is known to be exhausted nothing is sent and a stale cached answer is served if there is one.
  Secondary limits (403/429 with `Retry-After`) are waited out up to 20 s. Error bodies are reduced to
  GitHub's `message`/`errors` and redacted. File versions and merge bases come from the local clone
  when it has the commits (`git cat-file`, `git merge-base`), else the contents/compare APIs, or
  raw.githubusercontent.com when anonymous on github.com (free of the API quota). A pull request's
  diff base is the merge base of its `base.sha` and head, merged or not (a merged head already contained
  in `base.sha` keeps `base.sha`).
- **`commit_ci_status`** combines check runs (`filter=latest`), legacy commit statuses and the
  commit's workflow runs (newest per workflow and event) into one state: `failed` > `running` >
  `pending` > `manual` (waiting for approval) > `canceled` > `success` > `skipped`; conclusions map
  `neutral`→success, `timed_out`/`startup_failure`→failed, `stale`→skipped. `pipelineId` is the workflow
  run that explains the state. A short sha is resolved locally first. A commit GitHub does not know
  (unpushed; check runs answer 422) is `None`, as is `commits/{sha}/checks` (`null`).
- **REST** under `/api/projects/{pid}/github/`: `summary`, `rate`, `branch?name=`,
  `commits/{sha}/checks`; `actions/runs` (branch, status, event, workflowId, page), `actions/runs/{id}`
  (with jobs and steps), `…/{id}/rerun|rerun-failed|cancel`, `…/{id}/artifacts` (the run's uploads) and
  `actions/artifacts/{id}/zip` (streamed through; GitHub redirects to its blob storage, followed without
  the token; downloads need a token even on public repositories, expired ones are refused; the run panel
  lists them under the jobs), `actions/jobs/{id}` (+ `/rerun`,
  `/annotations`, `/logs` (`?tail=&plain=`), `/log` download), `actions/workflows`,
  `actions/workflows/{id}/inputs?ref=` (the file's `workflow_dispatch` inputs) and `/dispatch`;
  `pulls` (state open|closed|merged|all, search), `pulls/{n}` (GET, PATCH incl. `draft` via GraphQL)
  with `/files`, `/file?path=&previousPath=&status=&base=&head=`, `/threads` (+ `/{id}/resolve`, GraphQL,
  token only), `/review-comments` (+ `/{id}/replies`), `/reviews` (APPROVE, REQUEST_CHANGES, COMMENT),
  `/comments`, `/commits`, `/merge` (`sha` required, merge|squash|rebase, optional branch delete);
  `issues` (+ `/{n}`, PATCH, `/comments`), `releases`; global `/api/github/status`.
- **Logs.** GitHub publishes a job's log only once the job has finished, and only to signed-in users;
  `logs` then answers `available: false` with `reason` `running` | `needs_token` | `gone`, and the job
  panel shows the steps and check annotations instead. Timestamps and the BOM are stripped;
  `##[group]`/`##[endgroup]` and message markers stay for the viewer, which also folds each step
  (the server marks where steps start from the line timestamps).
- **Events** (with `projectId`): `github.run {runId, workflowId, name, state, status, conclusion, branch,
  sha, event, runNumber, webUrl, action, previousState?}` from actions here and from the poller (only
  while UI clients are connected: 30 s / 10 s while something runs with a token, 10 min / 5 min anonymous
  and only for projects whose GitHub data the UI or an agent asked for in the last 15 min, paused when
  the quota is low); `github.job {jobId, runId, action}`, `github.pr {number, action}`,
  `github.issue {number, action}`.
- **MCP tools:** `github_runs`, `github_run_jobs`, `github_job_log`, `github_prs`, `github_pr`,
  `github_pr_diff`, and (mutating) `github_create_pr`, `github_pr_comment`, `github_rerun`.
- **UI.** Tool window `github` (right, order 12, when `project.github`): Actions, Pull requests, Issues,
  Releases. Panels `gh.run`, `gh.job`, `pr`, `gh.issue`. The top-bar CI widget and status-bar item show
  only when GitHub is the project's forge (a project with GitLab too shows GitLab's). Anonymous views
  do not poll: they refresh on events and their Refresh buttons (the summary falls back to 10 min).
  Phone tab `github` (order 42) has the same four lists and takes `gh.run`, `gh.job`, `pr` and `gh.issue`
  panels (switching to their project; a pull request there has no side-by-side diff). Panels, the tool
  window, the phone tab and the dialogs load on first use. Detection: `Project::github()` takes a remote whose host
  has `github` in it, or (`projects::adopt_configured_forge`) a remote on the host config.toml's
  `[github] host` names (`git.corp.example`), with that configured host and an empty token; the same
  goes for `[gitlab] host`. Any other Enterprise host needs `[repo.github]` in the overlay.

**Agent providers.** `AgentInfo.provider` is the CLI kind: `claude` (default for old records),
`codex`, `kimi` or `custom`; `AgentInfo.providerId` names the configured provider (`claude`,
`codex`, `kimi`, `aider`…). `sessionId` is `''` until Codex's or Kimi's id is discovered, and
always for custom CLIs.
- **Config.** `[agents]` keeps the Claude Code defaults. `[agents.providers.<name>]` has `kind`,
  `command`, `args` (extra), `enabled`, `label`, `model`, `effort`, `permission_mode`, `env`
  (plain values such as `CODEX_HOME`) and `install_hint`. `claude`, `codex` and `kimi` are
  built-in presets; any other name is a custom CLI (`command` required), or another instance
  of a kind (`kind = "codex"`). `[agents].default_provider` picks the provider for `ask` and
  MCP. Project `[agent]` model/effort/permission apply to Claude only; its overlay `env` and
  `add_dirs` apply to every kind. A dangerous preset (Codex `bypass`, Kimi `yolo`/`auto`) is
  never a default: only a request selects it, and the UI confirms it. A `model`, `effort` or
  `permission_mode` the kind cannot take (an effort for Kimi, a model for a custom CLI, a
  dangerous default) is dropped with a `providerWarnings` entry, so it never fails every
  start; so is a `default_provider` that names no enabled provider (the composer's help
  button turns into a warning).
- **REST.** `POST /api/agents` and `/api/agents/ask` take `provider`; `ask` without one uses
  the most recent session of any provider that can take a prompt unasked: Claude Code when its
  state accepts one; Codex and Kimi only when idle after a turn of that process ended, with
  none of their dialogs on screen; custom CLIs never. Otherwise it starts a new session. A named
  terminal (`terminalId`) is refused (409) while its state or screen shows a dialog, and so is
  `POST /api/terminals/{id}/input` without `force`. `GET /api/agents/history?provider=` lists a
  provider's past conversations (`HistoryEntry.provider`, `lastMessage`).
  `GET /api/agents/defaults` adds `providers[]` (availability, install hint, what each
  supports, efforts, permission presets, defaults), `defaultProvider`, `providerWarnings`.
- **Workspace folders.** Claude, Codex and Kimi sessions get `--add-dir` for
  `data_dir/workspace/<project>` and `data_dir/workspace/home` (created 0700); Codex's
  `--add-dir` makes them writable roots of its sandbox.
- **Claude Code:** unchanged (hooks, status line, transcript).
- **Codex** (verified against codex-cli 0.157.1 `--help` and source): `codex [resume|fork]
  [--no-daemon] [--no-alt-screen] -c mcp_servers.workbench.url=… -c
  mcp_servers.workbench.bearer_token_env_var="WORKBENCH_AGENT_TOKEN" -c
  mcp_servers.workbench.http_headers={…} [--model] [-c model_reasoning_effort=…]
  [--sandbox/--ask-for-approval | --dangerously-bypass-approvals-and-sandbox] [--add-dir…] [<id>]
  [-- prompt]`. Optional flags come from the installed version's `--help` (cached);
  `--no-daemon` keeps the session in our process, since a shared daemon would not see its
  environment. Its rollout (`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*-<id>.jsonl`, local
  dates, created at the first turn) is found among files created after the launch whose
  `session_meta` names the cwd: the one our own processes hold open (`/proc/<pid>/fd`; on
  Windows the Restart Manager), or else the only one while no other Codex session of that home
  and cwd waits for its id, and only if no process outside the waiting sessions holds it open
  or runs Codex in that folder (a Codex in another terminal or an editor writes rollouts there
  too; such a file is never taken). It
  is tailed for `task_started` / `task_complete` / `turn_aborted`, model, effort and context.
  Approval prompts, questions and the trust prompt are not in the rollout: their texts (as
  0.157.1 prints them) are recognized on the bottom of the screen, and the session needs
  permission or input, with an attention event, until they are gone. Its `notify` hook
  is not used: it is legacy, and `-c notify` would replace the user's own. The history list
  judges each rollout by its first line and summarizes only the project's.
- **Kimi Code** (`@moonshot-ai/kimi-code` 2.1.1): `kimi [--session <id>] [--model] [--plan |
  --yolo | --auto] [--add-dir…]`. The prompt is pasted once it is up. The id comes from
  `$KIMI_CODE_HOME/session_index.jsonl` (`kimi::assign`): an entry of the cwd is possible for a
  waiting session when it was not in the index at its launch; a session gets the entry its own
  screen shows, or its only possible one when that is no other waiting session's only one
  (repeated, so sessions started one after the other all resolve; simultaneous ones need a
  screen). Without a screen, only while no Kimi outside the hosted Kimi sessions runs in that
  folder. History comes from the index and each session's `state.json`. There is no MCP flag
  and no fork.
- **Kimi and custom CLIs:** state is the output-activity heuristic (`terminals/activity.rs`):
  working while output flows, idle after a quiet spell. It never marks an answer unread.
  Kimi's approval panel, question panel and trust prompt, and a custom CLI's yes/no prompt on
  its last line (`[y/N]`, `(Y)es/(N)o`), are recognized on screen like Codex's. Custom CLIs
  cannot resume: a restart starts them over.
- Pastes wait 500 ms before Enter for Codex and Kimi (300 ms otherwise). An initial prompt that
  is pasted waits while the session shows a dialog (a trust prompt nobody answered yet). Workbench
  terminals drop the `CODEX_*` session variables of a Codex that started Workbench.

**Broader detection** (`apps/detect/`). It is still pure, bounded and read-only: files are
parsed, and nothing is executed, not even `just --list`. New ecosystems propose runs of kind
server, test, build or task, with ports and ready lines where they are knowable:
- Python (`python.rs`): the runner comes from the lockfile or tool section (`uv run`,
  `poetry run`, `pdm run`, `hatch run`, `pipenv run`, `rye run`), else the virtualenv's
  interpreter or `python3 -m`. It adds script tables, pytest or unittest, ruff, mypy, tox, nox,
  Django, FastAPI/Starlette/Litestar through uvicorn, Flask and Streamlit. Notebooks are
  recorded as components.
- Go modules: main packages, ports read from source, air, golangci-lint.
- CMake: a plain `build/` dir, or presets filtered by their `condition`; `ctest`; executables.
- Gradle and Maven (Spring Boot, Quarkus, Micronaut, Android), Ruby, PHP and Elixir.
- Task files (`tasks.rs`): Makefile, justfile, Taskfile and Procfile entries.
- JavaScript extras: deno tasks, turbo and nx roots, more dev servers, and ports read from a
  Node server's entry file.
- `compose.rs`: local compose stacks and Dockerfile builds. A compose file that describes a
  server is skipped: one under `deploy/`, a `prod` variant, or one a deploy script names or runs.
- Git (`git.rs`): a GitHub remote gives `repo.github {host, path}` with an empty token.
  GitHub Actions give `repo.ci` with provider `github`, unless `.gitlab-ci.yml` exists.
  Bitbucket, Codeberg and Gitea remotes are only tagged.

Rules for these runs:
- A body that reaches another machine or publishes (`ssh`, `rsync`, `docker push`,
  `docker compose push`, `DOCKER_HOST=ssh://`, `docker --context`, `deploy`…) puts the run in
  group `deploy`. This covers Make recipes, npm scripts and other task bodies, and everything
  an entry runs in the same file (`TaskGraph`): npm `pre`/`post` hooks and `npm run x`, Make
  prerequisites, `$(MAKE) x` and simple variables, just dependencies and `just x`, Taskfile
  `deps` and `task: x`, Python task sequences and `ref`, Composer `@x`, `deno task x`.
  `needsConfirm` also holds for any run whose command reaches out.
- Names from repository files (targets, task keys, script names, directories) enter
  commands as one shell word (`detect::sh`); a name with a control character is not offered.
- Files below `SAMPLE_DIRS` (`examples/`, `third_party/`, `testdata/`, `fixtures/`…) propose
  runs only when the rest of the project proposes none.
- A name clash in one directory is qualified by the tool (`serve · uv`).
- Commands are written in the run shell's language (`detect::dialect`, the one helper
  every Windows form goes through; `util::os::shell::Dialect`): POSIX for `bash -lc`, so
  Linux gets exactly what it always did. On Windows (PowerShell): the venv's
  `Scripts\python.exe`, `python` or `py -3` for `python3`, `a; if (-not $?) { exit … };
  b` for `a && b` (Windows PowerShell 5.1 has no `&&`; the failure keeps `a`'s status,
  127 when not found), `.\build\Debug\app.exe` after `cmake --build build --config
  Debug` (Visual Studio, CMake's default there, keeps a folder per configuration; the
  generator is the preset's, else the build dir's `CMakeCache.txt`, else
  `CMAKE_GENERATOR`), CMake presets for `Windows`, `.\gradlew.bat` and `.\mvnw.cmd`,
  `ruby bin/rails`, `php vendor/bin/phpunit`, the Unity editor in `%ProgramFiles%` called
  with `&`. Procfile lines and documented commands (`detect::repository_command`) in
  POSIX syntax (`$VAR` but `$PORT`, which becomes `$env:PORT`; `&&`, `VAR=x cmd`, `.sh`,
  `bash validate.sh`…), `wget`, and scripts started by their path without a Windows
  program beside them are not offered there; `python3` and `curl` become `python_words`
  and `curl.exe`. A run whose quoted words (names from repository files) hold one of
  `% ! ^ & | < > "` is not offered on Windows either: detected tools are often batch files
  (`composer.bat`, `mvn.cmd`), whose arguments cmd.exe reads again. Deploys and probes for
  an ssh host stay POSIX; a local `via_host` probe runs `curl.exe -o NUL`.

## Database (db)

The Database tool window (right; CLion's Database view) and SQL consoles, for PostgreSQL
(`tokio-postgres`; TLS with rustls).

**Data sources** are `[[database]]` entries of the project config: `name`, `host`, `port`,
`database`, `user`, `password` (a secret *name*), `url` (a secret name whose value is a whole
`postgres://…` or `key=value` URL, e.g. `{ dotenv = { path = ".env", key = "DATABASE_URL" } }`; the other
fields override its parts), `sslmode` and `read_only`. Unset: host `localhost`, port 5432, user
the OS user, database the user. Without a password `~/.pgpass` is read with libpq's rules (and
only when it is not readable by others; on Windows, like libpq, without that check). `sslmode`: `disable`, `prefer` (default) and `require`
encrypt without checking the certificate, as libpq does; `verify-full` checks it and the host name
against the system's roots (rustls-platform-verifier). `read_only` starts sessions with
`default_transaction_read_only`: a guard against slips, not a permission. The Add / Edit dialog
writes the machine overlay's `[[database]]` tables in place with `toml_edit` (`db/sources.rs`:
comments and other sections kept; names only, never values; a source from the repository is
replaced by an overlay copy and cannot be deleted from here).

**REST** under `/api/projects/{pid}/db`: `GET` → `{sources (with origin overlay | repository),
secretNames, overlayPath}`; `PUT|DELETE _sources/{name}` (`{source, previousName?}`); `POST
{name}/test` → `{version, ssl, user, ms, display, sslmode}` (a fresh connection); `GET
{name}/schema` → schemas with tables, views, materialized views, partitioned and foreign tables
(`pg_catalog`, estimated rows, at most 5000); `GET {name}/table?schema=&table=` → columns (type,
nullability, default, primary key, comment), indexes, foreign keys; `POST {name}/query {sql,
console, maxRows?}`; `POST {name}/consoles/{console}/cancel`; `DELETE {name}/consoles/{console}`.

**Sessions** (`db/query.rs`): one connection per console (`BEGIN`, `SET` and temporary tables
last across runs; closed after 15 idle minutes, when the source's settings or secrets change, or
with the panel), plus one for the tree (`_meta`). One query at a time per console (409 while one
runs). Queries use the simple protocol: every value as text, one result per statement of a
script, NULL as `null`, notices (RAISE NOTICE…) collected per run, errors with SQLSTATE, detail,
hint and position. **Row cap** (500 by default, up to 10,000; 10 KB per value, 8 MB per answer):
rows past it are read and dropped for up to 2 s or 50,000 rows, then the query is cancelled. A
PostgreSQL cancel stops whatever runs when it arrives, so it is only sent for results still
streaming; one that may still be in flight (the query ended first, or the request went away
mid-query) is absorbed by a short `pg_sleep` before the console's next query.

**UI** (`features/db`): the tree (sources → schemas → relations → columns with the primary key
marked, indexes, foreign keys; source menu: Open / New Console, Test Connection, Refresh, Edit,
Remove; table: double-click for its first 100 rows, Count Rows, Copy Qualified Name), the data
source dialog (secret pickers; Save and Test), and the `db.console` panel: a Monaco SQL editor
(text kept per console in this browser), Ctrl+Enter runs the statement at the caret or the
selection (`splitStatements` knows quotes, `E''`, dollar quoting and comments), Ctrl+Shift+Enter
the whole console; results in a grid with row numbers, sticky header, right-aligned numeric
columns, row selection and Ctrl+C / Copy as TSV; a tab per result set; the error's position as
an editor marker; DDL refreshes the tree. Agents get no database access yet.

## Dev containers

`server/src/devcontainer/**` and `web/src/features/devcontainer/**`. Workbench stays on the
host; a project with a `devcontainer.json` can have its container built and started on the
host's Docker, and the project's shells, run configurations and (when asked) agent sessions
then run **inside** it, in the host PTY through `docker exec`. The files stay shared through the
workspace bind mount, so the editor, git, search and the file watcher keep working on the host.

**Configs** (`config.rs`, `jsonc.rs`). Discovery: `.devcontainer/devcontainer.json`,
`.devcontainer.json`, `.devcontainer/<name>/devcontainer.json` (several: the panel picks one,
saved per project); symlinks leaving the project are ignored. JSONC (comments, trailing
commas). Supported: `name`, `image`, `build {dockerfile, context, args, target, cacheFrom,
options}` (and the legacy `dockerFile`/`context`), `dockerComposeFile` + `service`,
`runServices`, `workspaceFolder` (default `/workspaces/<basename>`, `/` for compose),
`workspaceMount`, `mounts` (string and object forms), `runArgs`, `containerEnv`, `remoteEnv`,
`remoteUser`, `containerUser`, `updateRemoteUserUID`, `forwardPorts` (numbers and
`service:port`), `appPort`, `portsAttributes` (labels), `overrideCommand`, `shutdownAction`,
`init`, `privileged`, `capAdd`, `securityOpt`, `features` (CLI engine), `initializeCommand` and
the lifecycle hooks (string → `/bin/sh -c`, array → exec, object → parallel). Variables:
`${localWorkspaceFolder}`, `${localWorkspaceFolderBasename}`, `${containerWorkspaceFolder}`,
`${containerWorkspaceFolderBasename}`, `${localEnv:VAR[:default]}`, `${containerEnv:VAR[:default]}`
(remoteEnv, resolved in the container) and `${devcontainerId}` (the CLI's algorithm). A config is
parsed twice: for display and the approval hash `${localEnv:…}` stays as written (host values
never reach the browser); for execution it is resolved and those values are masked in the output
(values of 8 characters or more: the terminals' redaction floor, as for run secrets).

**Trust** (`plan.rs`). The config, its Dockerfile and compose files are repository content, and
starting runs repository-defined code on the host's Docker. Therefore:
- Nothing is built or started by itself (not on project load, not on Workbench start, not by
  detection, which only records a `devcontainer` component and tag).
- `POST …/devcontainer/start|rebuild` needs `approve: <hash>`; without the current hash it
  answers `409 approval_required` with the plan. The hash is sha256 over the display config, the
  engine, the raw `devcontainer.json` and the contents of the files it uses (Dockerfile, compose
  files and every file compose reads through them: `include:`, `extends: {file}`, `env_file`,
  `.env`, secret/config `file`s, the Dockerfiles builds use); approvals are kept per project and
  config in `data_dir/devcontainer/<id>.json`. Any change asks again ("changed since you approved it").
  Scripts a Dockerfile `COPY`s and runs are part of the repository and not hashed.
- The confirmation lists the engine, the image or Dockerfile (context, target, build args,
  `build.options`), the workspace mount, mounts, `runArgs`, privileges, ports, users, variable
  names, features and every lifecycle command with when it runs. Risks are graded: **danger**
  (privileged, `--network/--pid/--ipc/--uts/--userns/--cgroupns=host`, host paths outside the
  project including `/` and `~/.ssh` from `${localEnv:HOME}`, the Docker/containerd/podman socket,
  devices, `--volumes-from`, `capAdd` of SYS_ADMIN, SYS_PTRACE, NET_ADMIN, ALL…, unconfined
  seccomp/AppArmor/labels, `initializeCommand` (it runs on the host), a Dockerfile, context or
  compose file outside the project, an included/extended compose file, `env_file` or secret file
  outside the project or named by a variable, compose nesting past 8 levels or 64 files),
  **warning** (lifecycle commands, builds, third-party or local features, `${localEnv}` values,
  `--env-file`, ports published on every interface, other capabilities) and **info**. Dangers need an extra tick before the button is enabled.
  Compose files are followed the way compose resolves them: relative paths in every
  `dockerComposeFile` against the first file's folder (the compose project directory), in an
  extended file against its folder, in an included one against its `project_directory`; every
  service reached (all services of named and included files, the chain a service extends) gets
  the rules above.
- Only the user acts: every write route refuses in-process callers (MCP tools), and the MCP tool
  `devcontainer_status` is read-only (it omits the approval hash).

**Engines** (`engine.rs`). `[devcontainer] engine = "auto"` (default) uses the built-in Docker
engine for image, Dockerfile and compose configs and the devcontainer CLI for `features`;
`"docker"` / `"cli"` force one. The CLI is `[devcontainer] cli` (a path, or `npx` for `npx -y
@devcontainers/cli`), else `devcontainer` on PATH; docker is `[devcontainer] docker`, else `docker`
on PATH. Either way the server writes a bash script (`data_dir/devcontainer/<id>/up.sh`, 0700)
from the approved plan and runs it in a Command terminal (`meta.devcontainer: true`), so the
build/up output streams where the user sees it; every command is echoed first and bounded by
`timeout`. Status probes (`ps`, `inspect`, `exec`) are `tokio::process` calls with timeouts.
- **Built in:** pull or `docker build -f … -t wbdc-<id>-<hash8> [--target] [--build-arg…]
  [--cache-from]`; `updateRemoteUserUID` builds a small layer matching the non-root remote user's
  UID/GID to the host's (as the CLI does); `docker run -d --name wbdc-<id>-<hash8>` with VS Code's
  labels (`devcontainer.local_folder`, `devcontainer.config_file`, `devcontainer.metadata` when
  users are set) plus `workbench.project` and `workbench.config-hash`, the workspace mount,
  mounts, `--env-file` (containerEnv and `-e K=V` runArgs, 0600, deleted at once), ports
  published on **127.0.0.1 only** (the same host port when free, else any), `--init`,
  `--privileged`, caps, security options, `-u containerUser`, the filtered `runArgs` (`--rm`,
  detach/attach/tty flags and `--cidfile` are dropped; the rest passes as approved) and the
  CLI's keep-alive entrypoint when `overrideCommand`. Compose: `docker compose -p
  <folder>_devcontainer -f … -f <override> up -d --build <service> <runServices>` with an override
  carrying the labels, environment, keep-alive and privileges.
- **Lifecycle:** create hooks (`onCreateCommand`, `updateContentCommand`, `postCreateCommand`)
  once per container, then a marker (`/var/lib/workbench-devcontainer/created`); a reused
  container of Workbench's without the marker (an earlier start failed) runs them again;
  `postStartCommand` when the container was (re)started; `postAttachCommand` on every Start.
  Hooks run as `remoteUser`, else `containerUser`, else the image's `devcontainer.metadata`
  user, in the workspace folder, with `remoteEnv`.
- **CLI:** `devcontainer up --workspace-folder --config --docker-path [--remove-existing-container]`;
  its JSON result names the container, remote user and workspace folder. It handles features,
  compose and lifecycle hooks itself (and writes its lockfile like VS Code does).
- **Reuse.** Containers are found by `devcontainer.local_folder` (the project root, or a path
  that resolves to it): one made by VS Code or the CLI is picked up (remote user from
  `devcontainer.metadata`, workspace folder from its bind mount). It is not used for terminals
  until the user starts/attaches or turns "Run terminals and runs in the container" on.
- Stop: `docker stop` (compose `stop` when `shutdownAction` is `stopCompose`). Remove: `docker rm
  -f` (compose `down`, volumes kept). Images are kept. Workbench leaves containers running when it
  exits.

**State.** `none | stopped | running | building | error` per project, from one `docker ps` of
labelled containers and one `inspect`, refreshed on demand (every GET), by a `docker events`
watcher (container and image events; a dev container's event refreshes the projects, any event
but `exec_*`, attaches and copies also becomes `docker.changed` for the Services tool window),
and every 15 s while UI clients are connected; changes emit `devcontainer.state
{projectId, state, containerId?, inContainer}`. `ProjectSummary.devcontainer` is `{configs,
state, inContainer}` or null.

**Running inside** (`exec.rs`). With the container running and "use the container" on (default:
on once Workbench started or attached to it; per project in the data dir), new shells and run
configurations get `meta.inContainer`, and `Terminals::launch` wraps them at every start:
`docker exec -it -u <remoteUser> -w <mapped cwd> [-e names…] <container> /bin/sh -c <wrapper>
<argv>`. Host paths under the workspace mount map to container paths (the cwd, and variable
values under the root, also inside `:`-lists such as `CARGO_TARGET_DIR`). Values never go into
argv: the docker CLI's environment carries them as `WB_E_<NAME>` and the wrapper exports them;
`remoteEnv` values travel as templates whose literal parts are single-quoted, so
`${containerEnv:PATH}` expands inside and nothing else is evaluated. Login shells (`bash -l`, a
run's `bash -lc`) apply the variables after the profile (Debian's `/etc/profile` resets `PATH`),
and `PATH` entries of the image's `ENV` the profile dropped are appended. A shell is the container
user's login shell from `/etc/passwd` (bash, else sh for `nologin` users); runs use `bash`, or
`/bin/sh` without it. Detach keys are moved off Ctrl+P. Killing `docker exec` leaves the process
in the container running, so the wrapper records its pid and kill/close/restart/shutdown signal
its process group inside (HUP, TERM, KILL). A container terminal whose container is gone fails
to start with that reason. Per terminal and per run the host stays available: New shell ▸ Host
shell (`POST /api/terminals {container: false}`), a run's menu ▸ Always run on the host
(`PUT …/devcontainer/settings {hostRun}`); runs whose folder is outside the project run on the host.

**Ports and readiness.** For a run inside, readiness probes connect to the **container's
address** (a published port's docker-proxy accepts connections before anything listens inside);
`ready.http`, the default URL and URLs the run prints (`http://0.0.0.0:8000/`) are rewritten to how
this computer reaches the port: the published `127.0.0.1:<hostPort>`, else the container IP
(Linux bridge; `reach: container-ip`, shown in the panel), or loopback for `network_mode: host`. A
busy port inside is reported as such and never `fuser -k`ed. Service `status`/`stop` commands
and `port:N` dependencies stay on the host.

**Agents inside.** The composer offers **Run in dev container** when the container runs and the
provider's CLI is on the container user's login `PATH` (`command -v`, probed per container).
Claude Code inside: hooks and MCP reach Workbench through the **bridge listener**
(`bridge.rs`) on the container network's gateway (`172.17.0.1` for the default bridge; compose
networks have their own), on the server's port when free there, else an ephemeral one; it runs
only while a container Workbench uses runs. It serves **only** `/api/hooks/**` and `/mcp` (404
for anything else) and only with an agent token (the master token and cookies are refused; Host
is rewritten for host pinning). `WORKBENCH_URL` inside is that listener; the per-session settings
and MCP JSON are written into the container (`/tmp/workbench-session-<id>/`, 0600, the exec user's,
through stdin); the token reaches the process only through the environment. Degradations: the
transcript lives in the container, so state comes from the hooks alone (no transcript tail: the
title and context come only as far as hooks report them; history lists show host sessions only);
no status line (the `workbench statusline` helper is a host binary); `SessionStart` is posted by
`curl` when the container has it (else a pasted prompt waits up to 60 s); resuming checks the
transcript inside. Workspace deliverable folders are not mounted (agents use
`workspace_write_file`). Codex, Kimi and custom CLIs inside run with the output-activity
heuristic, without Workbench's MCP. Claude's login inside is the user's own (log in there once,
or mount credentials in `devcontainer.json`); Workbench never mounts host credentials.

**Create devcontainer.json…** (`scaffold.rs`) proposes `.devcontainer/devcontainer.json` from
marker files (depth 3): the primary stack's `mcr.microsoft.com/devcontainers/*` image (Rust > Go >
Java > .NET > C++ > Python > Node), the other stacks as features (Rust + Node → the Rust image and
the Node feature), `forwardPorts` and labels from the project's run configurations,
`postCreateCommand` for dependency installs per folder (`npm ci`, `cargo fetch`, `uv sync`…) and
`remoteUser: vscode`. The UI shows it in an editable preview; `POST …/scaffold` writes it only
there (or `.devcontainer.json`, `.devcontainer/<name>/devcontainer.json`), must parse, and never
replaces a file.

**UI.** Panel `devcontainer` (config picker, state, Start/Attach, Stop, Rebuild, Remove, Open shell
in container, the "Run terminals and runs in the container" toggle, ports with how they are
reached, configuration, the graded review, agent CLIs inside, engines), the confirmation dialog,
a top-bar chip and a status-bar item for projects with a dev container, the Apps tool window's
entry and the phone's Apps tab (state, start, stop), palette commands (Show dev container, Start
dev container…, Stop dev container, Rebuild dev container…, Open dev container shell, Create
devcontainer.json…), `container` badges on terminal headers, tabs and session cards, and on runs.

**Services (Docker)** (`services.rs`, `features/devcontainer/services`). The bottom tool window
`services` (Alt+8, CLion's Services › Docker) lists every container of this computer's Docker,
grouped by compose project (the current project's first, then those with something running; a
"this project" filter keeps compose projects and dev containers whose folder is inside it), and
the images. REST under `/api/docker`:
- `GET containers` → `{containers, error?}` (one `docker ps -a` with a Go template: state, Docker's
  status words, ports, compose project/service/folder/files, `devcontainer.local_folder`, and the
  Workbench project that holds the folder). Docker not answering is `{containers: [], error}`,
  shown as the empty state.
- `GET containers/{id}` → inspected: command, times, exit code, health, restart policy, ports,
  networks, mounts, env and labels (masked, see Security model) and the whole `inspect` (masked).
- `POST containers/{id}/start|stop|restart|pause|unpause|kill`, `POST containers/{id}/remove
  {force?}` (the UI confirms; a running container needs `force`).
- `POST containers/{id}/logs|shell {projectId?}` → `TerminalInfo`: a `command` terminal following
  `docker logs --follow --tail 1000` (`meta.action = "logs"`, so it restarts), or `docker exec -it
  … /bin/sh -c 'exec bash, else sh'`. The terminal belongs to the container's project, else the
  caller's.
- `POST compose/{project}/start|stop|restart|down`: `docker compose --project-name`, run where no
  compose file is, so compose finds the containers by their labels (Down removes containers and
  networks; volumes and images stay).
- `GET images` (with the containers using each), `GET images/{id}`, `POST images/remove {ref}`
  (`docker rmi`: a tag, or a dangling image's id), `POST images/prune` (dangling only, never
  `--all`).

Ids, names, references and compose project names are validated (no leading `-`) before they reach
argv. The UI: a keyboard-driven tree (arrows, Left/Right collapse and expand, Enter = log, Delete =
remove), per-row context menus, and a detail pane with the actions and Info / Environment /
Labels / Inspect tabs (Monaco, read-only); published ports link to where the browser can reach them
(a loopback-bound port only from a browser on this computer); compose files inside a project open
in the editor. Nothing here runs by itself: lists load when the window is open (and every minute
while it is, so "Up 5 minutes" ages); changes arrive as `docker.changed`. Volumes and networks are
not listed yet.

**Limits.** Linux and a local Docker daemon (verified with rootful Docker 29; Docker Desktop's
VM hides container addresses: publish ports there). A host firewall that drops traffic from docker
bridges to the host blocks the bridge listener. Container IPs are reachable only on Linux bridge
networks. Root containers write root-owned files into the project (as with any devcontainer
without a remote user). Features, `hostRequirements`, `waitFor`, `userEnvProbe` and
`customizations` are not interpreted by the built-in engine (features need the CLI).
**Windows** has no dev containers (`util::os::support`: the bridge listener cannot bind the
gateway inside Docker Desktop's VM, and there is no uid mapping): `/api/projects/{pid}/devcontainer/**`
and `devcontainer_status` answer 501 `unsupported_platform`, `summary` is `None`, `running_target`
gives the reason and no container is polled. What asks for the container explicitly (a container
terminal or agent session, language servers' `container` mode) checks `require_supported` and
answers the same. Services works there and is marked experimental. The reason, getting-started
and Help point Windows users to the Linux build inside a WSL 2 distribution with Docker Engine
installed in it, where the gateway is a local address (a browser on Windows then opens only
published ports: `container-ip` links stay inside the distribution); with Docker Desktop's WSL
integration it may not be, and the bridge fails as on Windows (docs/windows-port.md §5;
neither is tested).

## Third phase (2026-09-27): code intelligence, debugger, CLion VCS, Confluence authoring, push, approvals, local history

Seven areas, built in parallel. Each owns its directories; the contracts below were fixed
before the build so the areas meet. Each area documents itself in its own subsection
below (routes, events, panels, trust, what is verified and what is not); their panels,
tool windows, events, REST prefixes, contracts, MCP tools and shortcuts are in the shared
tables above. "Integration" at the end says how the areas were wired together.

**New slices.** `lsp` (`server/src/lsp/**`, `web/src/features/lsp/**`) and `debug`
(`server/src/debug/**`, `web/src/features/debug/**`), with `[lsp]` and `[debug]` in
config.toml (`lsp::LspConfig`, `debug::DebugConfig`), REST under
`/api/projects/{pid}/lsp/**` and `/api/projects/{pid}/debug/**`, and `lsp::shutdown` /
`debug::shutdown` in `app::shutdown`. `[push]` (`platform::push::PushConfig`) belongs to
platform.

**Editor models (files contract, used by lsp and debug).** A Monaco model for a project
file has the URI `file:///<projectId>/<project-relative path>` (`~abs` instead of the
project id for absolute paths); only the files slice creates `file:` models
(`features/files/buffers.ts`). Other features reach models through
`features/files/modelAccess.ts` (`modelFile(uri)` is the one URI parser lsp, debug and git
use) and hook editors globally with `monaco.editor.onDidCreateEditor` / `onDidCreateModel`
(the namespace comes from `import('@/lib/monacoSetup')`). To show a file at a position, open the `editor` panel
(`openPanel({kind: 'editor', id: 'editor:<projectId>:<path>', params: {projectId, path, line,
column, t}})`); read-only views of files outside the project are the owning feature's own
panels.

**Trust.** Language servers and debug adapters run project code. Their commands come only
from config.toml (built-in presets plus `[lsp.servers.*]` / `[debug.adapters.*]`), never from
repository config. A project's language servers start only after the user enabled code
intelligence for it (remembered in `data_dir/lsp/`); a debug session starts only on a click,
like a run configuration. MCP tools never enable, start or approve anything.

**Permission requests from Workbench (terminals contract, used by platform push).**
- `AgentInfo.pendingPermission: {id, tool, summary, since} | null`: the Claude Code
  permission request a session is waiting on, when Workbench can answer it.
- `POST /api/agents/{terminalId}/permission {id, decision: 'allow' | 'deny', message?}` answers
  it. Devices only: agent tokens are refused (a session never approves itself). `409` when the
  request is no longer pending (answered in the terminal, timed out, or superseded).

**Push (platform).** `/api/push/**`, a service worker at `/sw.js` and `/manifest.webmanifest`.
A push for a permission request carries `{terminalId, permissionId}` so the notification's
Allow / Deny actions call the route above with the device's key.

### Code intelligence (lsp)

`server/src/lsp/**` and `web/src/features/lsp/**`: language servers bridged to the Monaco
editor for any language that has one. One server process per (project, server id), started
lazily, spoken to over stdio (`Content-Length` framing, `jsonrpc.rs`), shared by every
browser tab through one WebSocket per tab and project.

**Config** (`config.rs`). `[lsp] idle_minutes` (default 10) and `[lsp.servers.<id>]`
`{command, args, languages (Monaco ids), extensions, root_markers, enabled, label,
install_hint, env, initialization_options, settings}` (camelCase aliases accepted; TOML
tables become JSON). Built-in presets, in preference order (the first enabled, installed
server that handles a file by extension, else by language, serves it; servers of your own
come before the presets): `rust-analyzer`, `typescript` (typescript-language-server
`--stdio`), `pyright`, `basedpyright`, `pylsp`, `gopls`, `clangd`, `verible`
(verible-verilog-ls `--rules_config_search`, so a project's `.rules.verible_lint` applies;
Verilog and SystemVerilog), `vhdl_ls`, `bash`, `yaml`, `json`
(vscode-json-language-server), `taplo`, `marksman`, `csharp-ls`, `lua`. vhdl_ls reads the
project's `vhdl_ls.toml` (its libraries; without one it knows only the open files) and
stops when the IEEE libraries of its release are not next to its `bin/`. A preset's fields
are overridden one by one. Availability comes from PATH (or the configured path); a rustup
proxy (`~/.cargo/bin/rust-analyzer` → `rustup`) counts only when `rustup which
rust-analyzer` names a real binary, which is then run. The TypeScript preset gets
`tsserver.fallbackPath` from the `typescript` package installed next to the server
(typescript-language-server only looks in the workspace root, which fails for monorepos
with `web/node_modules`); TypeScript 7's native server (`tsc --lsp --stdio`) has no
`tsserver.js` and is configured as a server of your own. The LSP `languageId` comes from
the extension (`typescriptreact`, `shellscript`…). Servers start through `util::os::exe`:
on Windows an npm `.cmd` shim (typescript-language-server, pyright, bash, yaml, json)
runs as `node.exe` and its package script (whose folder `fallbackPath` is looked for
from), a rustup proxy is a hard link to `rustup.exe`, and another batch file runs only
with arguments cmd.exe cannot misread; absolute watcher globs (`C:\p/**/*.rs`) are
matched below their drive.

**Trust** (`trust.rs`). Nothing starts before the user enables code intelligence for the
project: `POST …/lsp/enable {mode?}` writes `data_dir/lsp/<id>.json` (0600, `{enabled, root,
enabledAt, mode, disabledServers}`; an approval for another directory does not count). The
UI asks with the Enable dialog (what runs, that it executes project code, where). Disable
stops the servers, closes the sockets (`{t:"disabled"}`) and clears diagnostics. Only
config.toml defines servers (`.workbench.toml` has no `[lsp]`; tested). Every write route
refuses in-process callers, so MCP tools cannot enable, start, stop or configure anything;
the socket also refuses them.

**Lifecycle** (`manager.rs`, `server.rs`). A server starts when a document it handles opens
in an enabled project, is initialized with the project root as its workspace folder and
the client capabilities Workbench implements (`server::client_capabilities`), then gets
`didOpen` for every open document of its language (under the project lock, so no change is
lost or doubled). It stops after `idle_minutes` with no open document and no request. A
crash restarts it after 1 s, then 4 s; the third crash within three minutes leaves it
`crashed` (notification; Restart in the popover). A start that fails (missing command,
container gone, initialize error) is `failed` with the reason and is retried with the next
document after a minute. Stopped by the user it stays off until restarted. The process
leads its own process group: stop sends `shutdown`/`exit`, then SIGTERM and SIGKILL to the
group (and `kill_inside` in a container); Workbench's shutdown and a removed or moved
project stop them. stderr, `window/logMessage` and lifecycle notes go to a 3000-line ring
per server, kept for the last process after it exits (`…/log`).

**Documents** are shared by every socket, reference counted per socket; the browser sends
full text (debounced 300 ms, flushed before every request on the document; 8 MB cap). Each
tab has its own buffers, so two tabs can hold different unsaved text for one file, while a
server has one text per URI (versions only grow): that of the tab **in use**, the last one
that sent a change, a save or a request. Every socket's own text is kept: a change, save or
request first hands the servers that tab's text of each document it has open (where another
tab's was theirs), so answers and edits (rename, code actions) match its buffers. Opening
a file never replaces another tab's text (a second window restoring its layout, a
reconnect); when the tab whose text the servers have closes the file or goes away, the most
recently used other tab's text takes over. Diagnostics go to the tabs whose text they were
computed for (the publish's `version`, else the current one) and to tabs without the file
open; a tab opening a file whose server text is another tab's gets empty lists (clearing
what it held), and a tab whose text becomes the servers' again without a new version (a
reload after a save elsewhere) gets the cached ones. The Problems window, the counts and
`code_diagnostics` show the servers' view (the tab in use). Servers that offer pull
diagnostics are asked 500 ms after a change (under the project lock, so the answer is
tagged with the version it is for). Diagnostics are cached per project (1000 per file, with
the version they are for); `lsp.diagnostics` carries the counts. Server requests handled:
`workspace/configuration` (from `settings`, by section or dotted path),
`client/(un)registerCapability` (file watchers), `window/workDoneProgress/create`,
`$/progress` (indexing %), rust-analyzer's `experimental/serverStatus`,
`workspace/workspaceFolders`, the `*/refresh` requests (forwarded), `window/showMessage`
(errors and warnings as toasts), `window/showMessageRequest` (a dialog in the most recently
active tab, 5 min, else no choice) and `workspace/applyEdit` (applied to that tab's buffers;
`applied: false` without a tab).

**Files changed on disk** that no browser has open (agents, checkouts, generators) reach
the servers as `workspace/didChangeWatchedFiles` from the files watcher's `fs.changed`,
filtered by the globs each server registered (created / changed / deleted; open documents
and directories skipped; an overflow is not expanded).

**URIs** (`uri.rs`). Browser models: `file:///<pid>/<rel>` (the files contract) and
`lsp-src://<pid>/<absolute path on the server's side>` (on Windows `lsp-src://<pid>/C:/…`,
and servers get `file:///C:/…`; `/c:/` and `/C%3A/` are read too, and the root matches
whatever the drive letter's case) for files outside the project that a
server pointed to (the standard library, `~/.cargo/registry`, `node_modules` through a
symlink, site-packages). Only URI fields are mapped, both ways: `uri`, `targetUri`,
`oldUri`, `newUri`, `baseUri` (Location, LocationLink, TextDocumentIdentifier,
publishDiagnostics, related information, inlay hint label parts, resource operations,
relative patterns) and the keys of `WorkspaceEdit.changes`. Text is never rewritten, even
when it is exactly a `file://` URI (a completion for a string literal type, its `newText`, a
hover, a diagnostic message), and `data` is left as the server wrote it (it comes back
verbatim in resolve requests). Outside paths named by those fields enter a bounded
per-project allow-set (50 000) that `GET …/lsp/source?uri=` requires (5 MB, UTF-8; read from
the host or with `docker exec … head -c` from the container). A symlinked or canonical root
maps too.

**Dev containers** (`launch.rs`). `mode`: `auto` (default: in the container when it runs and
the project's terminals use it, and the command exists there, else on the host),
`container`, `host`. In the container the server runs through `ExecTarget::wrap` without a
TTY (`-t` and the detach keys removed: a TTY would mangle the stream), as the container
user, in the mapped project folder; the command is found with `command -v` (and `rustup
which` for rust-analyzer); `env` values that are host paths the container cannot see are
left out; paths map through the workspace mount both ways; files only in the container are
served read-only from it. `processId` is null there. Verified with rust-analyzer in a
throwaway container: diagnostics, related information, navigation into the container's
standard library.

**REST** (`/api/projects/{pid}/lsp`): `GET` status (enabled, mode, dev container, every
server with state `off|starting|indexing|ready|stopped|crashed|failed|unavailable|disabled`,
progress, error, side, pid, command, availability and install hint, open documents,
`relevant` from root markers at the root or one level down, diagnostic counts, config
warnings); `POST enable {mode?}`, `POST disable`, `PUT settings {mode?, disabledServers?}`
(servers restart and tabs reopen their documents); `POST servers/{sid}/restart|stop`,
`GET servers/{sid}/log?tail=`; `GET diagnostics` (project files, most severe first, 5000);
`GET source?uri=`; `GET ws`.

**Socket** (`ws.rs`, `/api/projects/{pid}/lsp/ws`, selects on `auth.watch(caller).ended()`,
refused unless enabled). Browser → server: `open {uri, languageId?, text}` (→ `opened
{uri, server}` and the document's diagnostics), `change {uri, text}`, `close`, `save`, `req
{id, method, params, server?}` (routed by `params.textDocument.uri`, else `server`;
`workspace/symbol` without either asks every ready server and tags items `_server`),
`cancel {id}` (`$/cancelRequest`), `reply {id, result}`, `ping`. Server → browser: `hello
{servers}`, `caps`, `down`, `diagnostics {uri, server, diagnostics, version}` (to the tabs
whose text they are for, see Documents; after `opened`, one per server with cached
diagnostics, possibly empty), `res {id, server, result|error}`, `message`, `request {id,
method, params}`, `refresh`, `disabled`.
A socket's requests are cancelled when it closes; a socket that cannot keep up is closed
(1013) and resynchronizes. Per-method timeouts (10–120 s).

**Events.** `lsp.state {server, state, progress, transition}` (progress reports throttled to
400 ms), `{enabled}` and `{settings}`; `lsp.diagnostics {errors, warnings, infos, hints,
files}` (debounced 400 ms).

**MCP tools** (read-only; they only use servers that already run and never start one):
`code_diagnostics {path?}`, `code_symbols {query}`, `code_definition {path, line, column}`,
`code_references {path, line, column}` (1-based; a closed file is opened on the server for
that request only). Confined with `McpCtx::project_for`.

**Editor models contract** (`features/files/modelAccess.ts`, files slice, written by lsp):
`modelUriString(pid, path)` / `parseModelUri(uri)` (Monaco's encoding), `modelFile(uri,
projectOnly?)` (the file an editor's model shows, never a project root; debug and git use it
too), `modelUriFor`,
`peekModel` (open model, no loading), `readText` (buffer text with unsaved edits, else
disk), `ensureModel(pid, path) → {model, release}` (a buffer of the files slice; edits make
it dirty like any edit), `saveModel`, `isDirty`, `isReadOnly`, `onBuffersChange`,
`onBufferRevision` (saved or reloaded), `hasOpenBuffers`, and the path helpers
`isAbsolutePath`, `basename`, `samePath` (Windows drive paths, see "Editor models" under
TypeScript). `buffers.ts` exports `modelUri`.

**UI** (`features/lsp`). Providers are registered once for `file:` and `lsp-src:` models and
route each call to the model's server by capability (anything else answers nothing, so
Monaco's word completion and indentation folding stay): hover, completion (resolve,
snippets, additional edits, commit characters, label details; trigger characters are the
union of the ready servers', re-registered as servers come up), signature help,
definition / declaration / type definition / implementation (targets get short-lived
models so Ctrl+hover and peek can preview them), references, document highlights, document
symbols (outline, sticky scroll), formatting (document and range), code actions (applied
through Workbench, resolve, commands via `workspace/executeCommand`), inlay hints (refresh),
folding ranges, selection ranges. Diagnostics become markers (owner `lsp`, code links,
related information). Edits the servers make (rename, code actions, `applyEdit`) go into
the buffers of every affected file, opening a buffer for files no editor shows (released
once saved); the toast offers Save All; nothing is written to disk. Editors get actions
with CLion keys, added a microtask after `onDidCreateEditor` (Monaco fires it inside the
base constructor, before the standalone keybinding service exists):

| key | action |
|---|---|
| Ctrl+B / Ctrl+click | Go to Declaration or Usages (usages on the declaration itself; a chooser for several) |
| Ctrl+Alt+B | Go to Implementation(s) |
| Ctrl+Shift+B | Go to Type Declaration |
| Alt+F7 | Find Usages (tool window) |
| Ctrl+Alt+F7 | Show Usages (popup at the caret) |
| Shift+F6 | Rename… (preview of every edit, per file, before applying) |
| Ctrl+F12 | File Structure popup |
| Ctrl+Alt+L | Reformat Code (selection or file; failures reported, `null` answers point to the log) |
| Alt+Enter | Show Context Actions (quick fixes) |
| F2 / Shift+F2 | Next / Previous Highlighted Error |
| Ctrl+Alt+Shift+N | Go to Symbol… (palette command, works everywhere but terminals) |
| Alt+6 | Show Problems |

- Panel `lsp.source` `{projectId, uri, line?, column?, endColumn?, t?}`, id
  `lsp.source:<pid>:<path>`: a read-only library file, synchronized with the server so hover
  and navigation continue inside it; cross-file navigation goes through
  `monaco.editor.registerEditorOpener` (project files open the `editor` panel).
- Tool windows (bottom): `problems` (order 15; Current File / Project, severity filters,
  grouped by file, click to open) and `usages` (order 17; one tab per search, grouped by
  file with the line and the usage highlighted).
- Status bar: the project's servers (spinner and % while indexing, crashed/failed in red)
  and error/warning counts (click: Problems). Popover: enable/disable, where servers run
  (with a dev container), the servers serving the project's languages first (state,
  progress bar, command, pid, Restart / Start, Stop, Log, Off for this project), other
  servers with install hints, config warnings.
- Editor banner (overlay above a view zone): "Code intelligence is off" with Enable… / Not
  Now once per project; "not installed" with the install hint once per server and project.
- Dialogs: Enable, rename, server log (live), a server's question; popups: location
  chooser, File Structure, Go to Symbol.
- Palette: Go to Symbol…, File Structure, Show Problems, Show Find Usages, Code
  Intelligence…, Enable Code Intelligence….

**Verified**: Rust unit tests (framing incl. split reads and broken streams, id mapping
and cancellation, URI mapping host and container, allow-set, config and presets, rustup
proxies, trust gate, the launch command for containers, env filtering, watcher
classification, UTF-16 columns) and integration tests with a fake language server
(`src/lsp/testdata/fake_ls.py`: initialize, configuration, registration, progress,
diagnostics, hover, definition, references, rename, completion + resolve, workspace
symbols, custom requests, crash, `applyEdit`, cancel): nothing starts without enablement
(and not from MCP), repository config cannot add servers, the full editor protocol,
watched files, crashes with backoff then `crashed`, a server that cannot start, disable,
sign-out closing the socket, MCP tools; text that is a `file://` URI (completion label,
`newText`, command arguments, diagnostic message) comes back unchanged and does not make
the file readable, `data` round-trips verbatim; two tabs on one file (with and without
diagnostic versions): opening keeps the other tab's unsaved text, diagnostics reach only
the tab they were computed for, a request or change hands the servers that tab's text, a
closing or disconnecting tab hands over to the other; vitest for the conversions and UI
logic. In headless Chrome with rust-analyzer (standalone release, rust-src in scratch),
typescript-language-server (TypeScript 6), pyright: banner, Enable dialog, hover,
completion, signature help, diagnostics + Problems, navigation across files and into std,
the cargo registry and `node_modules`, usages, implementations chooser, rename preview and
apply + Save All, file structure, Go to Symbol, quick fixes, popover, log, missing-server
banner, both themes, and rust-analyzer in a dev container; with
typescript-language-server, a completion for a `"file:///etc/hostname"` literal type
inserts that text, and three tabs on one file with unsaved edits keep their markers on
their own lines. With the release binaries of Verible, vhdl_ls (0.88) and clangd (23) on
PATH: lint and syntax diagnostics as squiggles and in the status bar, definitions and
usages across files (a Verilog module instance, a VHDL function in another package), hover,
workspace symbols (vhdl_ls, clangd), a project's `.rules.verible_lint` switching a rule
off, and the `code_*` MCP tools on all three.

**Not done / limits**: with two tabs editing one file, the servers (and the Problems
window) follow the tab in use; an edit a server makes to a file the requesting tab does
not have open is computed on the servers' text, which may be another tab's unsaved one; a
server that sends no diagnostic `version` can, in a race with a switch between tabs, send
one batch to the wrong tab (the next batch corrects it). Code lens, linked editing,
`resourceOperations` (a server's create/rename/delete file edits are refused with a
message), one server per document (no eslint next to tsserver), rootMarkers only mark
relevance (the workspace folder is the project root), Monaco's own peek references show
no preview for files without a model (Alt+F7 / Ctrl+Alt+F7 are Workbench's).

**Hierarchy** (`HierarchyWindow.tsx`, right tool window `hierarchy`, order 50): Ctrl+Alt+H
(Call Hierarchy: callers or callees) and Ctrl+H (Type Hierarchy: supertypes or subtypes;
Monaco's Replace moves to CLion's Ctrl+R) run `textDocument/prepareCallHierarchy` /
`prepareTypeHierarchy`; each level loads when expanded (`callHierarchy/incomingCalls`,
`outgoingCalls`, `typeHierarchy/supertypes`, `subtypes`, routed to the server by
`params.item.uri`). A caller opens at its call site, a callee or type at its declaration; a
branch that reaches one of its own ancestors is marked recursive. Servers without the
capability say so (typescript-language-server has no type hierarchy; clangd has both).

**Semantic highlighting** (`semanticTokens.ts`): `textDocument/semanticTokens/full`
through one Monaco provider. Each server's legend is mapped onto the token types and
modifiers Workbench announces in `initialize` (aliases such as tsserver's `member` →
`method`; unknown types are dropped and positions re-encoded). Monaco gets that legend with
`variable` renamed `localVariable`, because theme rules match by name and the Monarch
`variable` rule would colour every local. `lib/monacoSetup.ts` colours types teal, functions
and methods blue, fields and enum members purple, macros and decorators like annotations
(from the `--syn-*` tokens of each theme); the whole-document feature is imported
explicitly (`register.all` only has the viewport one), and a document's features ask again
once it has its server (`onOpened`).

### Debugger (debug)

`server/src/debug/**` and `web/src/features/debug/**`: a CLion-like debugger over the Debug
Adapter Protocol, for any language with a DAP adapter.

**Adapters** (`adapters.rs`). Presets, in preference order: `gdb` (`gdb -q -i dap`, GDB ≥ 14;
C, C++, Rust, Fortran, Ada, D), `lldb-dap` (also `lldb-vscode` or a versioned `lldb-dap-NN`
on PATH), `codelldb` (`codelldb --port {port}`, TCP), `debugpy` (`python3 -m
debugpy.adapter`; on Windows `python`, else `py -3`), `delve` (`dlv dap --listen
127.0.0.1:{port}`, TCP). Adapters are found and started through `util::os::exe` (an npm
shim as node and its script; another batch file only with arguments cmd.exe cannot
misread). On Windows gdb reads only MinGW debug information: with an MSVC Rust
toolchain a gdb session says so and loads no pretty printers. `[debug.adapters.<id>]`
overrides a preset field by field or defines another adapter (`command` required): `kind`
(`gdb|lldb|codelldb|debugpy|delve|generic`, the launch-argument dialect), `label`, `command`,
`args`, `languages`, `transport` (`stdio|tcp`: `{port}` in `args` becomes a free loopback
port Workbench connects to), `enabled`, `env` (plain values, e.g. `PYTHONPATH`),
`adapter_id` (DAP `adapterID`), `launch_defaults` (merged into every launch/attach
request), `connect_timeout_s`, `install_hint`. `[debug] default_adapter.<language> = "<id>"`
picks the adapter per language; otherwise the first *available* one listing the language.
Availability is probed (cached 30 s): the command on PATH, `gdb --version` ≥ 14, `import
debugpy` for the configured interpreter; the UI shows the problem and the install hint.

**Trust.** Adapter commands come only from presets and config.toml. Repository layers may
define launch configurations (`[[debug]]`, they only run on a click, like run
configurations) and name an adapter by id, never a command; `restrict_repo_layer` tags them
with their layer (`source`), and `${secret:NAME}` in a repository launch configuration's
`env` resolves only against the overlay (`repo_secret_names`). Adapters start in a
directory of Workbench's own (`data_dir/debug/adapter`, `/` in a dev container), not in the
project: `python3 -m debugpy.adapter` puts its working directory first on `sys.path`, so a
repository's `debugpy/` or `platform.py` would otherwise run inside the adapter (and an
attach to an unrelated process would run repository code). Only gdb and delve *launches*
run in the launch's `cwd` (GDB < 15 has no `cwd` launch argument; `go build` finds the
module from its directory); gdb imports nothing from it. The DAP `cwd` argument still sets
the program's directory (`process::runs_in_project`). The session's `${secret:…}` values
are masked (`Session::redact`) in everything that shows what the program holds, not only
the console: variables, evaluate (every context), set-variable, completions, frame and
thread names, the stop's description and text, `source`/`file` contents and
`debug_state` — so the Variables view, watches, hover and "Ask agent" never see them.
Every write route
(`POST sessions…`, control, evaluate, set-variable, completions, breakpoint and watch
writes) refuses in-process callers: agents never start, step, evaluate in or stop a
session. Deriving configurations runs nothing (no `cargo metadata`: a
`rust-toolchain.toml` or `.cargo/config.toml` could run repository code).

**Launch configurations** (`launch.rs`, `derive.rs`, `config/project.rs`). `[[debug]]`
entries merge by `name` like `[[run]]`: `name`, `adapter`, `request` (`launch|attach`),
`language`, `program` (project-relative, on Windows with `/` or `\`, absolute or `~/`; `{root}`, toolchain
placeholders and `${workspaceFolder}` expand), `module` (Python `-m`), `args`, `cwd`, `env`,
`pre_launch` (alias `preLaunch`: a run configuration's name, started through the apps
slice and waited for until it exits 0 or is ready, or a command run in the run shell
(`bash -lc`; PowerShell on Windows) in a Command terminal; runs that need confirmation
are refused), `stop_on_entry` (alias
`stopOnEntry`; for gdb it means "stop at `main`": `stopAtBeginningOfMainSubprogram`),
`console` (`terminal` — the default where the adapter supports `runInTerminal` — or
`console`), `pid` (attach), `extra` (adapter arguments merged last). Derived ones, after
the explicit ones (explicit names win): Cargo targets of the root and of every Cargo
component detection found (`app/server`): binaries, examples, library unit tests and
integration tests, parsed from `Cargo.toml` files (members with `dir/*` globs, `autobins`…);
their pre-launch is `cargo build|test --no-run --message-format=json-render-diagnostics`
in a terminal with stdout redirected to a file (`data_dir/debug/tmp/<sid>.cargo.json`, or
`/tmp` in a dev container read back through `docker exec`), whose `compiler-artifact`
messages name the executable; CMake `add_executable` targets when a build directory with
a `CMakeCache.txt` exists (`cmake --build <dir> --target <name>`, then the newest
executable of that name in the build tree); Python scripts and modules from run
configurations (`python x.py`, `python -m m`, `uv|poetry|pdm… run …`, `pytest`, `uvicorn`…;
the run's or the project's `.venv` interpreter becomes debugpy's `python`); Go main packages
(root and `cmd/*`, delve `mode: debug`).

**Sessions** (`session.rs`, `client.rs`, `protocol.rs`, `process.rs`). A session is an
adapter process (own process group) or TCP connection, spoken to through `DapClient`:
`Content-Length` framing (noise before a header becomes adapter output), requests with
timeouts correlated by `seq`, events and reverse requests through a bounded channel to the
session's event loop. Start: pre-launch → adapter → `initialize` → `launch`/`attach` *sent
without awaiting it* (gdb answers only after `configurationDone`) → the `initialized` event
→ breakpoints, function breakpoints, exception filters → `configurationDone` → the launch
answer. gdb gets the program as an argument so breakpoints resolve at once, and for Rust
the default toolchain's pretty printers (`rustc --print sysroot` run outside the project
with `RUSTUP_AUTO_INSTALL=0`: a `rust-toolchain.toml` could name a toolchain inside the
repository, whose scripts gdb would load). A native attach is checked (`TracerPid`): gdb 17
answers `attach` with success when ptrace refused it; the error then explains
`kernel.yama.ptrace_scope`. On Windows (`util::os::support`) gdb neither attaches to a process
(an attach by language picks lldb-dap or CodeLLDB; one that ends up with gdb answers
`unsupported_platform`, a gdbserver `target` still goes) nor loads the pretty printers (the
console says so). Reverse requests: `runInTerminal` spawns a Workbench Command
terminal (`meta.debug`, `meta.debuggee`; redacted like the session's env), so the debuggee
has a real TTY; `startDebugging` starts a child session (`parentId`) with the given
configuration, sent as-is: when the configuration names a loopback `connect: {host, port}`
(debugpy's subprocesses) or the adapter talks TCP, the child opens a new DAP connection to
the parent's adapter (which knows the child process) — `initialize`, then `attach` with the
configuration; otherwise (a stdio adapter) it gets an adapter of its own. A non-loopback
address is refused, and so are connections inside a dev container (there debugpy launches
get `subProcess: false`, so subprocesses run undebugged instead of waiting forever). Child
sessions end before their parent: Stop terminates them (`terminateDebuggee` when Workbench
launched the tree), a parent that ends by itself detaches the ones still running. The UI's state per session: `starting|running|stopped|
terminated|failed`, `stopEpoch` (bumped on every stop, resume and `invalidated`: frame ids
and variable references of an older epoch are stale), `stopped {reason, description,
threadId, hitBreakpointIds (ours), duringEvaluation?}`, threads, exit code, process,
the capabilities the UI uses, the pre-launch and debuggee terminal ids. Stepping marks the
session running *before* the request, so a `stopped` event that overtakes the answer
wins. Run to Cursor adds a breakpoint only that session has, removed at the next stop.
Stop: `terminate` (launch, when supported) and `disconnect {terminateDebuggee: launch}`
(an attach detaches); the session then carries `stopRequested: true`, the killed program's
`exited` code is not recorded (the console says "Process stopped"), and the UI labels it
*Stopped* (an attach: *Detached*). An adapter whose stream ends while nobody ended the
session (no `terminated`/`exited`, no Stop) fails it: "<adapter> exited unexpectedly (exit
code N | killed by SIGKILL): <its last stderr lines>". Ending (`finish`, idempotent, awaited by concurrent callers): the
client is dropped (stdin EOF ends stdio adapters even when Workbench is SIGKILLed), a
debuggee we launched that is still the adapter's child is killed, the adapter's group gets
SIGTERM then SIGKILL, debuggee terminals and our pre-launch terminal are killed (a
pre-launch *run configuration* belongs to the apps slice and stays). `debug::shutdown`
stops every live session (8 s bound). At most 16 live sessions; 6 ended ones per project
are kept for their console (older ones are forgotten with `debug.session {removed: true}`). Console: DAP output plus `adapter` (its stderr / non-DAP
stdout), `workbench` and `repl-in|repl-out|repl-err` entries, redacted, capped (3000
entries, 2 MB), batched into one `debug.output` per burst of events.

*Evaluation stops.* A watch that calls a function can hit a breakpoint inside it; every
new stop re-evaluates the watches, which would stop again, forever. `evaluate` requests
are tagged, and the reader annotates events read while a tagged request waits with the tag
of the *oldest* one (lowest `seq`: adapters handle requests in order, so that is the one
running; `client::PENDING_TAGS`, the stream's order survives the split between responses
and events): such a stop carries `duringEvaluation: <expr>`, the UI shows why the program
stopped and evaluates that watch only on request from then on. The UI evaluates a
session's watches one at a time (`api.inTurn`), and a queued one whose stop epoch passed
is not sent.

**Dev containers.** When the project's terminals and runs use its running container
(`devcontainer::exec_target`), the adapter runs inside: its executable is looked up with
`devcontainer::agent_command`, the argv comes from `ExecTarget::wrap` minus `-t` (DAP is a
byte stream), the wrapper's pid file lets `kill_inside` end it, and pre-launch steps and
debuggee terminals get `meta.inContainer`. Paths map both ways in the server
(`PathMap`: breakpoints are sent with container paths, frames and output sources come
back as project paths), whatever the adapter supports. TCP adapters and attach are
refused inside a container (stdio adapters work). Verified with a throwaway container of
`mcr.microsoft.com/devcontainers/rust:1-bookworm` running the tests' fake adapter (program,
cwd and breakpoints sent as `/workspaces/…`, frames back as project paths, stderr apart,
env values outside argv, the adapter gone after Stop); that image's GDB is 13 (no DAP), so
a real debugger inside needs GDB ≥ 14 or lldb-dap in the image.

**Breakpoints** (`breakpoints.rs`) persist in `data_dir/debug/<project>.json` (0600):
line breakpoints (`id`, project-relative `path`, `line`, `enabled`, `condition`,
`hitCondition`, `logMessage`; one per line, 300 per file, 2000 per project), function
breakpoints, enabled exception filters per adapter id (absent: the adapter's defaults),
watches, `muted`, the last configuration started. A change sends `setBreakpoints` for that
file to every configured live session (an emptied file is sent empty once). Only enabled
ones are sent, only what the adapter's capabilities allow (a logpoint an adapter cannot do
is left out and reported unverified). Verification, per session from the answers and
`breakpoint` events (adapter ids map back to ours), is merged for the UI: verified when a
live session verified it, unverified with the adapter's message when sessions run and
none did, no status without a session. Exception filters an adapter announced are
remembered (per server run) so the Breakpoints view offers them.

**REST** under `/api/projects/{pid}/debug/`: `GET adapters`, `configs`, `processes` (this
user's processes from `/proc`, newest first, with `ptraceScope` and a hint); `GET|POST
sessions` (`{config, stopOnEntry?, pid?}`: an attach configuration that names no target —
no `pid`, no `connect`/`listen`/`target`/`waitFor`/… in `extra`, no program name for
lldb-dap/CodeLLDB — answers `400 pid_required`, and `pid` is the process the user picked;
Rerun reattaches to the same one), `POST sessions/attach {pid, adapter?, language?,
program?}`, `GET|DELETE sessions/{sid}`, `POST sessions/{sid}/stop|restart`, `POST
sessions/{sid}/control {action: continue|pause|next|stepIn|stepOut, threadId?}`, `POST
sessions/{sid}/run-to {path, line, threadId?}`, `GET sessions/{sid}/threads`,
`stack?threadId=&start=&levels=` (sources as `{path, inProject, name, sourceReference}`),
`scopes?frameId=`, `variables?ref=&start=&count=&filter=`, `source?ref=`,
`file?path=` (a file outside the project: only absolute paths the session's frames or
output named, a per-session allow-set; regular text files up to 5 MB, read in the dev
container when the adapter runs there; never Workbench's config or data directories,
config.toml's and the overlay's secret files, `~/.ssh`, `~/.gnupg` and similar credential
stores; devices only — agents read sessions through `debug_state`), `output?after=&limit=`; `POST sessions/{sid}/evaluate {expression, frameId?, context:
watch|repl|hover|clipboard}` (a `repl` evaluation is echoed into the console),
`set-variable {variablesReference, name, value}`, `completions {text, column, frameId?}`;
`GET breakpoints`, `PUT breakpoints/file {path, breakpoints}`, `PUT breakpoints/functions`,
`PUT breakpoints/exceptions {adapter, filters}`, `PUT breakpoints/mute {muted}`, `POST
breakpoints/clear`, `PUT watches {expressions}`. Adapter failures answer `422
debugger_error` with the adapter's message; a missing or unavailable adapter
`not_configured`; an ended session `409`.

**Events** (with `projectId`): `debug.session` (`SessionInfo`; `{id, projectId, removed:
true}` when forgotten), `debug.output {sessionId, lines}` (a flood sends 500 lines and the
UI fetches the rest by sequence), `debug.breakpoints` (the whole breakpoints view: lines,
functions, exception filters, muted, watches).

**UI.** Tool window `debug` (bottom, order 25): session tabs and the launch configuration
picker with Debug and Attach buttons on top; one row with the view tabs (*Threads &
Variables* — *Start* without a session —, *Console*, *Breakpoints*) and the session
toolbar (Rerun, Resume/Pause, Stop, Step Over/Into/Out, Run to Cursor, View Breakpoints,
Mute, Ask agent about this stop, the program's terminal, the state). Frames: thread
picker, stack (the first frame in the project within 15 is selected; a click opens its
source). Variables: an evaluate field (Enter evaluates, Ctrl+Shift+Enter adds a watch),
watches on top (evaluated per stop and frame), scopes (locals and arguments open; lazy
children; arrays over 200 elements in pages of 100; values changed since the last stop
highlighted; double-click sets a value when the adapter can; copy, add to watches).
Console: output by category, a REPL (gdb takes its own commands) with Tab completions and
history. Breakpoints: by file with enable boxes and edit/remove, function breakpoints,
exception filters, mute, remove all. Start: the configurations grouped (explicit, Cargo,
CMake, Python, Go) with their problems, Attach, and the adapters with availability and
install hints. A top-bar chip shows the current project's live session with Stop.
Dialogs: breakpoint properties (enabled, condition, hit count, log message); pickers in
the palette's look: *Debug…* (configurations) and *Attach to Process…* (substring filter,
ptrace explanation; opened for a launch configuration after `pid_required`, it attaches
that configuration to the process picked). Panel `debug.source` (`{projectId, sessionId,
path | sourceReference, name?, line?, column?, t?}`; id `debug.source:<projectId>:<path>`,
or `debug.source:<projectId>:<sessionId>:ref<n>`): a read-only Monaco view of a frame's
source outside the project (`file?path=`) or of source only the debugger has
(`source?ref=`, preferred whenever a frame has a `sourceReference`, as DAP says), with the
execution point and the selected frame; its models are `inmemory://debug-source/<sid>/…`
(never `file:`), hovering evaluates in its session. Frames and stops open it for every
frame that is not a project file. Editor (through Monaco's global hooks, see "Editor models"; Monaco is
hooked once it has loaded): the glyph margin (turned on in the files slice's editor)
carries breakpoints — red dot, dotted when conditional, amber log point, hollow grey
disabled, hollow red when a live session could not place it, grey when muted —, the
execution point (arrow and line) and a selected outer frame; a click toggles, Ctrl+click
enables/disables, Shift+click adds a log point, right-click opens the menu (edit, disable,
remove, conditional/log point, run to cursor, view breakpoints); a hover dot previews a new
breakpoint; breakpoints follow edits (decorations tracked, saved 0.7 s later); hovering an
identifier or member chain while suspended evaluates it (`hover` context, no calls).
A stop brings its source to the front and shows the tool window (not on a phone, whose
shell has no editor tab: no toast per stop there). The frame it shows: after a step or at a
breakpoint the top one when it has source (a step into a library shows the library); after
a pause, a signal or an exception the first project frame within 15. "Ask agent about this
stop" pastes the stop, the stack, the selected frame's variables and recent output into
an agent session (not submitted).

| Command | Shortcut |
|---|---|
| Debug (the picked or last configuration; with neither, the Debug… picker) | Shift+F9 |
| Debug… (picker) | Alt+Shift+F9 |
| Attach to Process… | Ctrl+Alt+F5 |
| Resume Program | F9 |
| Step Over / Step Into / Step Out | F8 / F7 / Shift+F8 |
| Run to Cursor | Alt+F9 |
| Stop Debugging | Ctrl+F2 |
| Toggle Line Breakpoint | Ctrl+F8 |
| View Breakpoints (on a breakpoint line in the editor: edit it) | Ctrl+Shift+F8 |
| Show Debug | Alt+5 |
| Pause Program, Rerun, Mute Breakpoints, Ask Agent About This Stop, Debug *config* | — |

F9, F8, F7, Shift+F8 and Ctrl+F2 are taken in the capture phase while the current
project has a live session (Monaco binds F8, Shift+F8 and Ctrl+F2 itself); terminals keep
every key. Ctrl+F8, Alt+F9 and Ctrl+Shift+F8 are editor actions (they need the caret);
the palette commands act on the last focused editor.

**MCP** `debug_state` (read-only): per session of the project (or `sessionId`) its
configuration, adapter, state, error, exit code, and when stopped the reason, threads,
the stack of the stopped thread (`#i function at file:line`, 30 frames), the locals of a
frame (`frame`, default 0; scopes marked expensive, registers, globals and statics left
out; 60 values of up to 300 characters; secret values masked) and the console's tail (the
newest 4000 characters of the last 40 entries). It only sends
`stackTrace`, `scopes` and `variables`.

**Verified** (Linux, GDB 17.1, debugpy 1.8.22 on Python 3.14): unit tests (framing,
correlation, timeouts, adapter config, breakpoint store and DAP mapping, Cargo/CMake/
Python/Go derivation, cargo JSON parsing, path mapping, process list); integration tests
against a fake adapter (`fake_dap.py`: launch deferred like gdb's, breakpoints and
verification, stepping, run to cursor, evaluate/set variable/completions, console,
`runInTerminal`, `startDebugging`, stop, pre-launch success and failure, MCP, agent
refusal, shutdown, evaluation stops); real sessions through the REST API and the browser:
a C program built by a pre-launch run configuration (breakpoints, conditional
breakpoint, log point, step over/out, set value, watches, console REPL and completions,
exit code, pause of a running program, stop), a Rust binary through the derived Cargo
configuration (pretty-printed `String`/`Vec`), a Python script through debugpy with the
program in its own terminal, attach (refused under ptrace_scope 1 with the explanation;
allowed with `PR_SET_PTRACER`; detach leaves it running), the fake adapter in a real dev
container. Review fixes (phase-3 review): integration tests for adapters started outside
the project (a `python3 -m` adapter with the project shadowing `json`, `socket`, `debugpy`;
launch and attach), secrets absent from every REST and MCP answer, Stop vs. an adapter
crash, pruning events, `pid_required` and Rerun of such an attach, child sessions over a
connection to the parent's adapter, the `file?path=` allow-set; the oldest-tag rule in
`client`; with real debugpy (`WORKBENCH_TEST_DEBUGPY=<PYTHONPATH dir>` enables
`real_debugpy_debugs_subprocesses`): a subprocess's breakpoint hits in a child session,
and stopping the parent ends the child and its process; in the browser (both themes):
Step Into a C function compiled from a file outside the project (the `debug.source` view
with the execution point), a debugpy `sourceReference` frame, a Python subprocess child
session, Shift+F9 opening the picker, an attach configuration asking for its process,
*Stopped*/*Detached* labels, pruned tabs leaving, no toasts on the phone. **Not verified:**
lldb-dap, CodeLLDB and delve (not installed here: presets and dialects only), a real
debugger inside a dev container, child sessions of a TCP adapter (js-debug style: the code
path is the one the fake adapter's loopback child uses), the phone (no mobile tab:
debugging is desktop-only).

### Version control: interactive rebase, line staging, changelists, shelf, bisect (git)

`server/src/git/{lines,rebase_i,changelists,shelf,bisect}.rs` and
`web/src/features/git/**`. Everything runs on the host's git CLI, like the rest of the
slice; every mutation takes the repository write lock and emits `git.changed`.

**Line staging** (`lines.rs`). `GitFileDiff` gains `canSelectLines` and `lines:
[{hunk, kind: 'add'|'del', line, at}]` (`line` on its own side; `at` = the modified-side
line a deletion shows at; empty above 50,000 changed lines). `POST
…/git/{stage,unstage,discard}-lines {path, fingerprint, lines: [{kind, line}]}` re-runs the
diff (`working` for stage/discard, `staged` for unstage), refuses a changed fingerprint
(409) or a line that is not a change (409), and builds a minimal patch from the parsed
hunks: forward (stage) keeps selected changes, turns unselected deletions into context
and leaves unselected additions out; reverse (unstage, roll back; applied with `-R`)
does the mirror image. Positions are exact on the side the patch applies to and
shifted by the emitted hunks' delta on the other; `-N,0` hunks follow git's convention.
A line without a final newline stays last on its side: when turning a change into
context would put lines after it, the change is kept and a copy with a newline is
emitted (the smallest valid patch). Lines are raw bytes (CRLF and any encoding
round-trip). A working tree git checks out with CRLF over an LF index (`core.autocrlf`,
`text`/`eol=crlf` attributes): git's diffs already show it with LF, so the staging patches
are LF and `git apply` writes CRLF back when rolling back. On every OS Workbench follows git
for any file git reads with LF although it has CRLF on disk: the working-tree side
(`modified`, a conflict's `merged`) is shown with LF and a conflict resolved with edited
text is written back with CRLF (`eol.rs` reads `git ls-files --eol` and `core.autocrlf`,
only for a file with a CRLF in it: an LF file costs no git call). Besides the checkouts
above, that is a file saved with CRLF over an LF index under `text`, `text=auto` or
`core.autocrlf=input`, and CRLF committed as is under a `text` or `eol` attribute (git shows
every line changed until it is renormalized). Files git does not convert (`-text`, CRLF
committed as is under the automatic conversions, no conversion configured) keep their
bytes. Part of an untracked file becomes a `new file` patch; an intent-to-add
entry gets a modification patch; renames patch the new path; every line of a new/deleted
file becomes the file-level operation; partial roll back of a deleted file and partial
unstage of a staged deletion are refused. Binary, LFS, symlink, submodule and conflicted diffs offer no lines.

**Partial commit** (CLion's line checkboxes). `POST …/git/commit` takes `partial: [{path,
fingerprint, lines}]` next to `paths`: lines of the HEAD → working tree diff (`compare`
mode, `base=HEAD`, empty `head`). The commit is built in a temporary index holding HEAD
(whole files added, selected lines applied with `git apply --cached`), committed with
`GIT_INDEX_FILE` pointing at it (hooks see that index), and the real index then takes
the committed version of those paths (`git reset -q HEAD -- …`), so what was not
selected stays as unstaged changes and other staged files stay out. Refused during a
merge and for the first commit. A rebase stopped at an `edit` step accepts commits
(amend or new ones). A renamed file (the diff has `oldPath`) is first renamed in the
temporary index (HEAD's blob and mode of the old name under the new one), then its lines
are applied, so part of a rename commits as a rename.

**Interactive rebase** (`rebase_i.rs`). `GET …/git/rebase/plan?from=<commit>|onto=<ref>` →
`{head, branch, base, root, onto, commits (oldest first: sha, subject, message, author,
time, pushed), skipped, dirty, merges, pushedRef, rewritesAll, state}`; `pushed` = on any
remote-tracking branch (the upstream or another one: a branch that tracks `origin/main` but
was pushed as `origin/topic` counts, like Undo Commit's check); `pushedRef` names the remote
branch holding the newest pushed commit (the upstream when it does). Onto a branch the
commits are listed like git's todo generator does (`--right-only --cherry-mark
<base>...HEAD`): commits already on the target as a cherry-pick (patch-equivalent, not
empty) go to `skipped` (git leaves them out, so they disappear from the branch; shown
struck through under the list) instead of `commits`. The commits the run rewrites start at
the first entry that is not an in-place `pick`, or earlier at the kept commit a
squash/fixup melds into (it is amended although its own entry is an unchanged `pick`);
pushed ones among them, plus pushed skipped commits, need `confirmPushed`.
`POST …/git/rebase/interactive {from|onto, head, entries: [{sha, action, message?}],
autostash, confirmPushed, opId?}` → `202 {opId}`. Refused: HEAD
moved (409), a merge in the range (flattening), a dirty tree without autostash
(`409 dirty_tree`), rewriting pushed commits without `confirmPushed` (`409 pushed`),
anything but a clean state, an edited plan that is not the planned set, a squash/fixup
without a kept commit above it, an empty reword. It runs as a git op (`git.op`, op
`rebase`, the write lock held) with `GIT_SEQUENCE_EDITOR` = `workbench git-editor todo
<staging>` and `GIT_EDITOR` = `workbench git-editor message <git dir>` (a new CLI
subcommand, `main.rs` → `git::cli_git_editor`): the todo helper checks that git's todo
lists exactly the planned commits (else the rebase stops before anything happens),
copies the messages into `<git dir>/rebase-merge/workbench/` (they live exactly as long
as the rebase) and writes our todo; the message helper finds the step in
`rebase-merge/done` (a `reword`, or the last step of a chain containing a `squash`) and
writes the prepared message, or leaves git's. The run uses a `core.commentChar` no line
of any message starts with, so `#123` lines survive. The staging folder
(`data_dir/git/rebase/<pid>-<rand>/`) is removed when the op ends. Stops (`edit`,
conflicts) end the op with `stopped: true` (and `conflicts`), and use the existing
Continue / Skip / Abort; `continue` keeps using the message helper while such a rebase
runs (a reword that stopped on a conflict still gets its message). `GitStatus.stateDetail`
gains `interactive`, `edit` (stopped at an `edit` step) and `stopped` (sha).

**Changelists** (`changelists.rs`). `data_dir/git/changelists/<pid>.json` `{version, active,
lists: [{id, name, comment, created}], files: {path: listId}, seen: {path: {base, away}}}`,
keyed by project-relative path. Reconciled with the status on every `GET …/git/status`
and changelist read: changed tracked files without a list join the active one (new
changes go to the active list), unknown ids fall back to the active list; a missing or
corrupt file starts over. Untracked and ignored files are not in lists. A file whose
change leaves the working tree (stash, shelve, an autostash during Update Project or a
rebase stopped at `edit`) keeps its entry, marked `away` (not on a truncated status): when
the change comes back and HEAD still has the version it was made against (`base`, the
status's `hH` blob), it returns to its list; when HEAD's version changed meanwhile (the
file was committed, e.g. from a terminal) it is a new change and joins the active list.
Entries away longer than 14 days (or beyond 5,000, oldest first) are pruned; a rollback
in Workbench (`POST …/git/discard`) forgets the files at once. Routes: `GET|POST
…/git/changelists` (create `{name,
comment, active, paths}`; names unique case-insensitively, 409), `PATCH|DELETE
…/git/changelists/{id}` (rename, comment, `active: true`; deleting moves the files to the
active list, the last list stays), `POST …/git/changelists/move {paths, to}`. Event
`git.changelists {}`.

**Shelf** (`shelf.rs`). `data_dir/git/shelf/<pid>/<id>/meta.json` `{id, name, created, base,
branch, files: [{path, oldPath?, status, binary, patch, changelist?}], viewCommit?}` and one
`git diff --binary --full-index` patch per file under `files/`. Shelving builds the patch
in a temporary index (HEAD + the files' working-tree state, untracked files included,
renames paired), writes the shelf (staged under a dot folder, then renamed), checks that
the patch reverse-applies to the working tree, and only then rolls the files back
(`restore` from HEAD, or remove what HEAD lacks); `keep` saves a copy only. `GET
…/git/shelf` lists them (newest first), `GET …/git/shelf/{id}` adds `viewCommit` (a
dangling commit of the base + the shelved changes, rebuilt when gone, for the `commit`
diff panels), `POST …/git/shelf {name, paths|changelist, keep}`, `PATCH`/`DELETE
…/git/shelf/{id}` (rename, delete), `POST …/git/shelf/{id}/unshelve {paths?, remove,
changelist?}` → `{ok, conflicts, message, applied, conflicted, removed}` (shelving records
each file's changelist; without `changelist` unshelved files go back to that list when it
still exists, else wherever the changelists put them): `git apply
--3way`; modifications come back unstaged, new files and renames staged (like `git stash
apply`); conflicts leave unmerged entries for the conflict panel and keep the shelf; local
changes in the way answer 409. Event `git.shelf {}`.

**Bisect** (`bisect.rs`). `GET …/git/bisect` → `{active, termGood, termBad, bad, good, skipped,
current, start, remaining, steps, result, candidates, log}` (refs, `BISECT_*`,
`git rev-list --bisect-vars`; `result` from the log's `first bad commit`, both git
spellings). `POST …/git/bisect/start {bad?, good: [..]}` (good commits must be ancestors
of bad; a dirty tree in the way is reported as such), `…/bisect/mark {verdict:
good|bad|skip, rev?}` (custom terms honored), `…/bisect/reset`. No `bisect run`: nothing
executes by itself.

**Also**: `POST …/git/undo-commit {sha}` (CLion's Undo Commit: `reset --soft HEAD~1` for an
unpublished, non-merge, non-root HEAD; returns its message), `GET …/git/log?path=&lines=a,b`
(`git log -L`, "Show History for Selection"; `gitlog` panel params gain `lines`). With
`worktreeLines=true` the lines are working-tree line numbers (what an editor selection
has): they are mapped onto the logged revision's version through the revision → working
tree diff (`-U0`; a line in a replaced block maps to the old block, a line in an added
block to the old lines around it; 400 when every selected line is uncommitted), and a
file renamed since is logged under its old name. The editor action sends it (panel param
`worktreeLines`).

**Helpers and git's refusals.** `GIT_ASKPASS` for remote ops comes from
`util::os::helper::askpass_env` (`GitState.askpass` holds the environment): on Unix the
`data_dir/git-askpass` wrapper script as before; on Windows the absolute `workbench.exe`
itself with `WORKBENCH_HELPER=askpass`, also as ssh's `SSH_ASKPASS` with
`SSH_ASKPASS_REQUIRE=force`, so a passphrase or unknown host key fails the op at once
(remote ops start without a console), and `GCM_INTERACTIVE=never`, so Git Credential
Manager, which git asks first, never opens a sign-in window. For the GitLab host askpass
answers for, remote ops empty git's credential helper list on every OS, so Credential Manager
is neither asked for it nor handed Workbench's token (Security model, "Git credentials").
`main.rs` answers such a call before clap parses
anything: the variable set and a single argument that is not a subcommand or an option
(`askpass_prompt`), so hooks and the rebase editor, which inherit the variable, still run
their commands. `GIT_EDITOR`/`GIT_SEQUENCE_EDITOR` quote their paths for sh with `/`
separators (`rebase_i::sh_path`; Git for Windows runs them with its sh). On Windows
(`os::fs::FOREIGN_OWNERS`) a repository git refuses for its owner (`safe.directory`,
"detected dubious ownership") answers `403 unsafe_repository` with git's message verbatim
(it names the owners and the command that trusts the folder) instead of `not_a_repo`.
The check is core's `util::git::refuses`, which the other readers of a checkout share:
`util::git`'s `try_` queries say why git gave no answer (`Failure`: not installed, timed
out, refused, git's message, or a working folder that is gone, which is not a missing git
although the spawn fails with `NotFound`). `From<Failure> for ApiError` answers a missing
git, a timeout and a refusal as this slice does (`not_configured`, `timeout`,
`403 unsafe_repository`); every other failure, a folder that is no repository among them
(this slice's `404 not_a_repo`), is `422 git_error` with git's message. So a project summary
warns about a refused folder with the command that trusts it
(`util::git::refused_warning`), a deploy reports git's failure instead of "no commits", and
the forge pollers and CI summaries (`…_logged`) still leave the branch out but log a
refusal, a missing git or a timeout once per folder instead of dropping it silently. The UI
keeps the message's line breaks (`ErrorBox`), copies git's `git config --global --add
safe.directory …` line (`trustCommand`, through `@/ui`'s `copyText`, which falls back to
`execCommand` on plain HTTP), and the status bar shows "Untrusted repository" where the
branch would be.
Remote ops that fail because ssh would have had to ask (an unknown host key, a key with a
passphrase: `Permission denied (publickey)`) get a hint for doing that once in a terminal or
loading the key into an agent; a changed host key gets a warning to check its fingerprint
first, and a revoked one a warning not to trust it again (`remote::explain_failure`).

**UI.** Commit tool window: tabs **Changes / Stash / Shelf**; Changes groups by the
staging area (as before) or by **changelists** (toolbar ▸ Group by), with a checkbox per
file and list for what the commit includes (default: the active list; a partly included
file shows an indeterminate box and "partial"), drag files between lists, list and file
menus (set active, commit changelist, shelve, move to…, new/edit/delete). The diff panel:
select lines (editor selection or the Monaco context menu: Stage / Unstage / Rollback
Selected Lines), a line-checkbox mode in the glyph margins (Shift+click toggles the whole
change; forces side-by-side), and in the HEAD → working tree diff opened from the
changelist view the checkboxes choose the lines to commit (a compact "3/4 lines" badge,
Include / Exclude All). The diff toolbar wraps onto a second row in a narrow split.
Interactive rebase dialog (log ▸ Interactively Rebase from Here… / Edit Commit Message… /
Drop Commit…, branch menus ▸ Interactively Rebase 'x' onto 'y'…, palette): rows oldest
first, action select and P/R/E/S/F/D keys, drag or Alt+↑/↓ to reorder, inline messages
(one editor per squash chain, on its last squash row: git's combined message derived from
the current rows until the user edits it, then kept and flagged when the chain's commits
change, with "Use the Combined Message"; only that row sends a message), pushed markers,
skipped (already applied) commits, typed confirmation (the branch name) before rewriting
pushed commits, autostash. The op card of a stopped rebase
offers Continue / Abort (or Resolve Conflicts). Bisect: log menu ▸ Start Bisect: This Is
Bad/Good…, a banner (Commit window, log, phone) with progress, the commit under test,
Good / Bad / Skip / Reset and the result, and log markers (bad, good, skip, testing,
first bad; commits outside the range dimmed). Undo Commit… on HEAD in the log, Compare
with Branch… (file menus, diff toolbar), and **Git: Show History for Selection** in every
file editor's context menu (a global Monaco hook on `file:///<projectId>/…` models).
Palette: Shelve Changes…, Unshelve Changes…, New Changelist…, Interactively Rebase onto
Branch…, Undo Last Commit…, Bisect: Start… / Mark Good / Mark Bad / Skip / Reset. No new
shortcuts.

**MCP.** `workbench_changelists` (read-only: lists with their files, shelves with their
files). Nothing that stages, commits, shelves, rebases or bisects is exposed. The git tools
now resolve the project with `McpCtx::project_for` (a session names only its own project).

**Verified**: unit tests (patch building: forward/reverse, adjacent changes, zero-count
hunks, no newline at EOF both ways, CRLF bytes, new files, path quoting; todo generation,
chains, pushed counts incl. squash/fixup into a pushed commit, comment characters; the
editor helper; changelist reconcile, away/return/commit-meanwhile, expiry, cap and repair;
working-tree → revision line mapping; shelf listing and splitting; bisect parsing) and
integration tests on
throwaway repositories (line staging including CRLF/autocrlf, untracked, intent-to-add,
deleted and renamed files, stale fingerprints; partial commit; undo commit; real
interactive rebases through the helper: reword/squash/fixup/drop/reorder, edit stop +
amend + continue, conflict stop + continue keeping the reword, pushed/dirty/moved/merge
guards, pushed on another remote branch, fixup into a pushed commit, onto a branch with a
cherry-picked and an empty commit, autostash; partial commit of a renamed file; shelf
round trips with text, binary, new, deleted and renamed files,
partial unshelve, keep, 3-way conflicts, local changes in the way; bisect to the first
bad commit; `log -L` incl. working-tree lines and a staged rename; changelists over the
REST routes and MCP incl. stash/pop, a commit from a terminal, rollback and unshelving
into the recorded list). In the browser (both
themes, a depth-20 clone and a synthetic repo): line checkboxes and staging, a partial
commit end to end, the changelist view, a real interactive rebase with a conflict stop,
bisect to the result, shelve/unshelve, Show History for Selection, Compare with Branch,
Undo Commit, the phone banner. Not verified: huge repositories (plans cap at 1,000
commits, shelves at 10,000 files).

### Confluence authoring and Jira boards (atlassian)

`server/src/atlassian/{comments,files,pages,agile}.rs` and `web/src/features/atlassian/**`.
The aim is that the owner never needs Confluence in a browser: comment, attach, mention,
link, label, move, copy and delete from Workbench. Jira boards are general-purpose and
verified only against a mock server.

**Inline comments** (`comments.rs`, v2). A comment is anchored by
`inlineCommentProperties {textSelection, textSelectionMatchCount, textSelectionMatchIndex}`
(zero-based index; count > index). Confluence matches the selection against the rendered
page, so the server counts non-overlapping occurrences in the text of the same sanitized
`view` HTML the UI shows (`html::text_content`, `html::occurrences`); the UI counts in its
DOM (`confluence/selection.ts`) and sends both numbers. A different count means the UI shows
an older version: 409, nothing posted. A selection must stay inside one block (paragraph,
list item, cell, heading), never in code, have no line break and at most 1000 characters;
an ambiguous one (several occurrences, no index) is refused. Resolve / reopen and edits are
`PUT …/{inline|footer}-comments/{id}` with version + 1 and the body (the current one when only
the state changes); edits carry the version they started from (409 when it moved). Dangling
comments cannot be resolved (Confluence refuses). Deletes remove the thread.
`CommentOut` adds `markdown` (the body as markdown, mentions as `[@Name](mention:<accountId>)`),
`editLossy` and `resolvedBy`/`resolvedAt`. `comment_md.rs` writes that markdown to be parsed
again: text is escaped (`src/*.rs`, `__init__`, a paragraph starting `1. ` stay literal), `<br/>`
is a hard break (`\` at the end of the line), task lists are `- [ ]` items. `editLossy` is decided
by the round trip: `markdown::to_storage(markdown)` is compared with the original after
normalizing what carries no meaning (`local-id` and other ids, whitespace, `b`/`strong`, a `p`
inside a list item or not, table column widths); any other difference (macros, page links,
colours, a table without a header row…) is flagged in the UI. `markdown::to_storage` turns `mention:` links into
`<ac:link><ri:user ri:account-id=…/></ac:link>`, so agents and the comment composer can mention.
`GET …/comments?replies=false` skips the per-thread reply requests (the page view colours
highlights from it).

**Attachments** (`files.rs`). List: v2 `pages/{id}/attachments` (newest first, 500 cap,
trashed ones hidden), each with Workbench's proxy URL. Upload:
`POST /api/confluence/pages/{id}/attachments?name=&comment=&replace=` with the raw file as
the body, streamed browser → Workbench → Confluence as v1 multipart
(`POST|PUT /wiki/rest/api/content/{id}/child/attachment`, `X-Atlassian-Token: no-check`,
`minorEdit=true`): `Content-Length` is required (411), 100 MB cap (413 before anything is
sent), the stream refuses more bytes than announced, a 15-minute timeout, no retry on 429.
An existing name answers 409 `exists` unless `replace` (a new version through `PUT`). Delete:
v2 `DELETE /attachments/{id}` (to the trash). Downloads use the existing proxy
(`?download=1` forces a download).

**Page operations** (`pages.rs`). Labels: v1 `POST content/{id}/label` (lowercased; no
spaces or `!#&()*,.:;<>?@[]^`, checked before sending), `DELETE content/{id}/label?name=`.
Move: v1 `PUT content/{id}/move/{before|after|append}/{target}`; before/after a top-level
page is refused (Confluence's tree hides top-level pages). Copy: v1 `POST content/{id}/copy`
(destination `parent_page`, else the original's parent, or `space`; attachments and labels
optional; permissions, properties and custom content never). A top-level page (the space home)
has no parent to copy next to: without `parentId` or `spaceKey` the answer is a 400 naming the
page, and the Copy dialog asks for the page to put the copy under instead of offering "Next to the
original". Trash: v2 `DELETE pages/{id}`;
restore: v2 `PUT pages/{id}` with `status: current` on the trashed page (only the status
changes). Watching: v1 `user/watch/content/{id}` (GET/POST/DELETE). People:
v1 `search/user?cql=user.fullname ~ "…"` (names cached for rendering). `PageOut.users` gives the
names of the accounts a page mentions, for the editor's mention chips.

**Jira boards** (`agile.rs`, `/rest/agile/1.0`). Boards (paginated, 500 cap, by name or
project), a board's columns (configuration: column → status ids, WIP min/max) and quick
filters, sprints (`active,future,closed`; kanban boards have none: their 400 is "no
sprints"), issues of the board, of a sprint or of the backlog (`fields` = the search fields,
100 a page, 500 cap, optional JQL). Moving a card is a workflow transition:
`GET /api/jira/issues/{key}/transitions` lists them, the UI picks the one whose target status
is in the target column (a menu when several, a warning when none) and runs the existing
transition route, optimistically (rolled back on failure).

**Routes** (all under the existing prefixes):
`DELETE /api/confluence/pages/{id}` (trash), `POST …/pages/{id}/restore`,
`POST …/pages/{id}/inline-comments`, `PUT|DELETE /api/confluence/comments/{inline|footer}/{cid}`,
`GET|POST …/pages/{id}/attachments`, `DELETE /api/confluence/attachments/{att}`,
`GET|POST …/pages/{id}/labels`, `DELETE …/pages/{id}/labels/{name}`, `POST …/pages/{id}/move`
`{position, targetId}`, `POST …/pages/{id}/copy` `{title?, parentId?, spaceKey?, copyAttachments,
copyLabels}`, `GET|PUT …/pages/{id}/watch`, `GET /api/confluence/users?q=`;
`GET /api/jira/boards?name=&project=`, `GET /api/jira/boards/{id}`, `…/{id}/sprints?state=`,
`…/{id}/issues?sprintId=|backlog=true&jql=`, `GET /api/jira/issues/{key}/transitions`.

**Events.** No new types: `confluence.page {pageId, action}` gains the actions
`comment-updated`, `comment-resolved`, `comment-reopened`, `comment-deleted`,
`attachment-added`, `attachment-deleted`, `labels`, `moved`, `deleted`, `restored` (a copy is
`created`); `jira.issue` also refreshes board queries.

**UI.**
- Page view: highlights coloured by their comment (resolved ones plain, the active one
  stronger); selecting text shows **Comment** (or Ctrl+Alt+C, not while typing in a field),
  the passage stays highlighted (CSS Custom Highlight API) while the comment is written in the
  side pane.
- Side panes (one at a time, remembered): Comments (Page / Inline tabs; threads with reply,
  edit and delete of your own, resolve / reopen, "Show resolved", @mentions in the markdown
  composer) and Attachments (previews, upload by button or drop with progress and cancel,
  replace-as-new-version confirmation, download, move to trash). Both cover the page in a
  narrow panel (container queries).
- Comment drafts (`confluence/drafts.ts`, a zustand store mirrored to this tab's
  sessionStorage, 50 drafts, a week): the text of a new page comment, a reply, an edit or a new
  inline comment (with its anchor) lives there until posted or cancelled, so switching tabs,
  closing the pane or a failed refresh loses nothing; an inline comment left unsent shows as
  "Unsent inline comment" in the Inline tab and can still be posted (the server re-checks the
  anchor). An edit keeps the version it started from and saves against it: when the comment
  moved (seen on refresh, or the save's 409) the card shows their newer text above the editor
  with **Discard mine** / **Overwrite with mine** (a save against the version now shown).
- A refresh that fails while a page, its comments, its attachments or a Jira issue are on
  screen keeps them (and any edit) and shows "Could not refresh … Retry" above them; the error
  box replaces content only when nothing was loaded yet.
- Header: labels as chips (× removes, + adds). More menu: Move…, Copy…, Watch / Stop watching,
  Move to trash… (typed title; the toast offers Restore). Page tree context menu: Move up /
  Move down (among siblings), Move…, Copy…, Move to trash….
- Rich editor: `@name` → people → mention; `[[title` → pages → page link; Ctrl+K (the editor
  keeps the key, so the palette does not open) or the toolbar → a link dialog (page by title
  or a web address; the selection becomes the link text; edit / remove a link); paste, drop or
  the paperclip upload files to the page (clipboard images get `image-YYYYMMDD-HHMMSS.png`, a
  taken name gets `-1`, never replaced) and insert `<ac:image><ri:attachment …/></ac:image>` or an
  attachment link, with a placeholder chip while uploading. All of these are storage atoms kept
  byte for byte, so they survive edit round trips and render in view mode.
- Jira tool window: Issues | Boards tabs; the Boards tab lists boards (the project's
  `[links.jira]` project keys first) and opens panel **`jira.board`** `{boardId}`
  (id `jira.board:<boardId>`): sprint picker (active, future, closed, backlog, all), sprint goal
  and days left, quick filters as chips (combined with AND), a card filter, columns with WIP
  limits, statuses no column maps in "Other statuses", drag to transition. Palette command
  **Jira: boards**.
- Core addition: `api.upload(path, blob, query, onProgress, signal)` in `api/client.ts`
  (XMLHttpRequest, for upload progress; same device key and error mapping as `request`).

**MCP tools.** `confluence_add_inline_comment` (mutating; `selection`, `occurrence` 1-based,
count taken by the server), `confluence_upload_attachment` (mutating; a file inside the calling
session's project: relative or absolute inside the root, 50 MB; the answer includes the storage
markup to show it). Refused, checked both on the path as given and on the file it really is
once symlinks are followed (`docs/notes.txt -> ../.env`, a folder linked into `.git/`; the
canonical file is what is read): hidden files and folders, key and token names, and whatever
`files::Sensitive` withholds (its built-in credential names plus the project's `sensitive`
patterns, gitignore-style, so `secrets/` also covers `config/secrets/…`), `confluence_labels` (mutating: lists, adds,
removes), `jira_boards`, `jira_board_issues` (read: columns with their issues; `sprint` =
active | backlog | board | an id).

**Verified.** `mock_tests.rs` (the mock now models inline comments with anchoring, comment
versions, v1 multipart uploads, labels, move, copy, trash/restore, watching, people and Jira
Software boards/sprints/backlog/transitions): every write path above, the refusals (stale
counts, stale versions, bad labels, top-level siblings, oversized or unannounced uploads,
secrets through MCP, including symlinks to them and nested `sensitive` folders) and all five
tools; `comment_md.rs` tests the escaping, hard breaks, task lists, tables and what is flagged
lossy, and that comments written as markdown edit back to the same storage. In the browser against the mock: selecting and
commenting with a mention, resolving, editing, attachments (upload, new version, delete),
the editor (mention, `[[`, Ctrl+K, pasted image, save → the storage the mock received),
labels, move, copy, trash and restore, the board (drag-transition, quick filter, backlog), both
themes; after the review: an edit racing another device's save (conflict shown, a stale save
refused, overwrite and discard), an edit racing a Confluence-side change (409), drafts kept
across tab switches, closing the pane and a site that stopped answering (stale notices, no
error box), an unsent inline comment posted after reopening the pane, Copy… of the space home
(asks for a parent, copies under it), and a comment with globs, `__init__`, `1. `, a hard break
and a task list saved back unchanged. Against a real Confluence Cloud site, GET only: inline and footer comments (dozens of detached
inline comments on one page), attachments with previews, labels, watching, people search,
the editor on a page with 26 page links, and Jira reported as not configured.
**Not verified live** (no writes to the real site): the exact match counting Confluence
applies to `textSelection` (documented only loosely by Atlassian; mismatches surface as
Confluence's 400), whether `PUT inline-comments` needs the body to change state, v2 restore
and v1 copy, all Jira Software calls. Not built: ranking cards within a column, estimates on
cards, footer-comment resolution (the v2 API has no field for it).

### Phone app and push, service install (platform)

`server/src/platform/push/**`, `server/src/platform/service.rs` (`service_windows.rs` and
`src/bin/workbenchw.rs` on Windows), `web/src/features/platform/**`
(`push.ts`, `pushLib.ts`, `sections/Push.tsx`), `web/public/**` (`manifest.webmanifest`,
`sw.js`, `icons/`), `web/scripts/icons.mjs`, `packaging/windows/workbench.ico`. Workbench on a
phone behaves like an app and reaches the owner while it is closed: "Claude needs your permission"
with Allow and Deny on the lock screen.

**Installable app (PWA).** `/manifest.webmanifest`: `id`/`start_url`/`scope` `/`, `display:
standalone`, `background_color` and `theme_color` equal to the dark `--bg` and `--bg-panel` tokens
(literals are unavoidable there), icons 192/512 (`any` and `maskable`) and an SVG, shortcuts
**Agents** (`/?tab=agents`) and **Files** (`/?tab=files`). `index.html` links the manifest and
`apple-touch-icon`; `meta[name=theme-color]` starts at `--bg-panel` and follows the theme
(`PushBridge` reads the token after each theme change). The icons are drawn from one geometry (the
favicon's) by `node web/scripts/icons.mjs` (no dependencies; generated PNGs are committed):
`icons/workbench.svg`, `icon-192/512.png`, `maskable-192/512.png` (glyph inside the 80 % safe
circle), `apple-touch-icon.png` (180, full bleed), `badge-96.png` (monochrome notification badge),
and, outside the web bundle, `packaging/windows/workbench.ico` (PNG images of 16–256 px: the icon
`server/build.rs` embeds in the Windows executables with a version resource, through the
build-dependency `winresource`; a build without a resource compiler only warns). The same
resource holds the application manifest `packaging/windows/workbench.manifest`: Windows 10/11
as `supportedOS`, Common Controls 6 (message boxes), `longPathAware` and `asInvoker`, in the test
executables too, where a `cfg(windows)` test in `os::autostart` checks that Windows applies it.
`spa.rs` serves `/sw.js` as `text/javascript` with `no-cache` and its own CSP (`spa::SW_CSP`:
`default-src 'self'`, same-origin fetches only) and the manifest as `application/manifest+json`
with `no-cache`; the SPA's CSP is unchanged (`worker-src 'self'` covers the registration). A launch
URL's `tab` (phone: the saved tab; desktop: that tool window) and `open` (below) are read once at
load and removed from the address bar.

**Service worker** (`web/public/sw.js`, scope `/`, registered by the platform provider with
`updateViaCache: 'none'`). It has **no fetch handler and caches nothing**: `/api`, `/view` and
`index.html` always come from the network, so an update never serves a stale page (Chrome no
longer needs a fetch handler to install). It handles:
- `push`: a JSON payload (below) → `showNotification(title, {body, tag, renotify, icon, badge,
  timestamp, data, requireInteraction})`, with actions **Allow** / **Deny** when the payload has
  `terminalId`, `permissionId` and `allow: true` (**Review** / **Deny** without `allow`). Nothing is shown while a Workbench window is visible
  (`clients.matchAll`; the server's presence rule, for a push that raced it), except in Safari,
  which revokes subscriptions whose pushes show nothing.
- `notificationclick` with `allow`/`deny` (an `allow` only for a payload with `allow: true`;
  `review` opens the session like a tap): `POST /api/agents/{terminalId}/permission {id,
  decision}` with `credentials: same-origin` and `X-Workbench-Key` read from IndexedDB
  (`workbench`/`kv`/`deviceKey`, written by the page when push is on or synced, deleted when it is
  turned off or the phone signs out). 2xx → a silent "Allowed · Bash" / "Denied · Bash" in place of
  the request, cleared after 4 s. 409 (Workbench can no longer answer it: answered in the terminal,
  the session moved on, or Workbench's `permission_wait` ran out while the terminal still asks) →
  "No longer pending here · Bash" without actions, which stays until dismissed and opens the session
  when tapped (it may still wait at its prompt). 404 → "This session is gone", tapping opens the
  Agents home; another 4xx → "Not answered" with the status. Neither offers Allow / Deny again.
  401/403 → Workbench is opened to sign in. 429, 5xx or no network → the request again, with its
  actions and the reason.
- `notificationclick` on the body: focus a Workbench window and `postMessage({type:
  'workbench:open', target})`, or `clients.openWindow('/?open=<target JSON>')`. The page opens only
  known kinds (`terminal`, `agents.home`, `pipeline`, `gh.run`, `settings`) after switching to the
  target's project when it exists (`pushLib.parseTarget`), since any page can link `/?open=`.
- `pushsubscriptionchange`: subscribe again (`vapidKey` in IndexedDB) and `POST
  /api/push/subscriptions`; the server keeps the device's topics.

**Web Push server** (`push/`).
- **VAPID** (RFC 8292, `vapid.rs`): a P-256 key pair generated once into `data_dir/push/vapid.json`
  (0600, directory 0700). An unreadable file is moved aside (`vapid.json.bad-<ms>`) and replaced; a
  new key drops the stored subscriptions (they are bound to the old one) and devices subscribe
  again when opened (`pushLib.syncAction`). Every request carries `Authorization: vapid t=<ES256
  JWT>, k=<public key>` with `aud` = the endpoint's origin, `exp` = now + 12 h (a token is reused
  while it has an hour left) and `sub` = `[push] subject`, else an https `server.public_url`, else
  the https origin the device subscribed from, else `mailto:workbench@localhost`.
- **Encryption** (RFC 8291 over RFC 8188 `aes128gcm`, `ece.rs`): a fresh sender key and salt per
  message, one record (rs 4096), payloads up to 3993 bytes (ours stay under ~1.2 KB).
- **Endpoint allowlist** (`endpoint.rs`): the endpoint comes from a browser and the server POSTs to
  it, so it is an SSRF boundary. Accepted: `https`, default port, a DNS name (no IP literal), no
  userinfo or fragment, ≤ 2048 characters, host `fcm.googleapis.com`,
  `updates.push.services.mozilla.com`, `web.push.apple.com`, `<one label>.notify.windows.com`, or
  `[push] extra_endpoint_hosts` (exact, or `*.example.com` for subdomains). Checked when
  subscribing and again before every send; the push client follows no redirects and is https-only.
  Only `cfg(test)` code can allow `http://127.0.0.1` (the tests' mock push service); there is no
  runtime switch.
- **Subscriptions** (`data_dir/push/subscriptions.json`, 0600): one per **device session**
  (subscribing again replaces it and any other record of that endpoint), at most 64, with the
  device's topics and "hold while in use elsewhere". A subscription ends with its session: the
  auth hook `AuthState::sessions_ended()` (a watch bumped when sessions end) plus an hourly check of
  `session_active` (expiry) prune them, and every delivery re-checks.
- **Delivery.** Headers `TTL` (3600 s; the test 120 s; a permission request with Allow / Deny: no
  longer than it stays answerable, i.e. `pendingPermission.since` + `[agents] permission_wait` − now,
  0 once past, so a phone that comes back online later is not offered a request Workbench can no
  longer answer), `Urgency` (`high` for an agent that waits,
  an environment going down or a failed deploy; `low` for a recovery), `Topic` (32 characters of the
  tag's SHA-256, so the push service replaces an undelivered older message about the same thing),
  `Content-Encoding: aes128gcm`. 201/2xx → delivered; 404/410 → the subscription is dropped;
  429/500/502/503/504 and connection errors → up to 3 attempts, waiting `Retry-After` (≤ 30 s) or
  2 s then 6 s, abandoned when a newer note with the same tag was sent meanwhile; 400/401/403/413
  → recorded, not retried. Upstream bodies are never echoed: errors are the status with a short
  reason, or the root network cause (no URL: the endpoint is a capability URL and never logged).
  At most 8 sends at a time, counted per attempt: a retry's wait holds no slot, so a push service
  that is down does not hold up deliveries to the others. The test button makes one attempt and
  returns what the service said.
- **Coalescing and presence.** Notes wait 700 ms per tag (`agent:<terminalId>`,
  `env:<pid>:<env>`, `deploy:<terminalId>`, `pipeline:<pid>:<ref>`, `notify:<terminalId>`); a
  newer note with the same tag replaces the waiting one. The page reports
  `POST /api/push/presence {visible, active, tab}` on visibility changes, every 30 s while visible
  and on `pagehide` (`active`: input in the last 2 minutes), only once some device has push. `tab`
  is the page's own random id (every tab of a browser shares one device session): presence is kept
  per session and tab (at most 32 tabs per session), so one tab hiding, reloading or closing leaves
  the others; a report without `tab` counts as one tab of its own. A device with a tab seen visible
  in the last 75 s gets no push; a device with "hold while in use elsewhere" (default on) gets none
  while a tab of another device is visible and active.
- **Triggers** (`notify.rs`, so desktop and push stay consistent): a push is queued for a desktop
  note that passes the notifier's rate limiter: `agent.attention` (topic `attention` for
  `needs_permission`/`needs_input`/`error`, `done` for a finished turn), environment down/up
  (`env`), a finished deploy (`deploy`), a failed GitLab pipeline or GitHub workflow run
  (`pipeline`, opening its `pipeline`/`gh.run` panel) and `workbench_notify` (`notify`). An agent
  note is sent only if, when its window passes, the session is still in the state that caused it
  (answered at the desk meanwhile → nothing), and for `needs_permission` it carries the session's
  `AgentInfo.pendingPermission` (read from `TerminalInfo` as JSON, per the terminals contract; absent
  → a plain notification). A `terminal.updated` showing a new `pendingPermission.id` while the
  session waits pushes it too (a second request without a new attention event); each permission id
  is pushed once.
- **Payload** (JSON, titles and short summaries only: no code, file contents or secrets, except
  that a permission request offering Allow carries the request itself, at most 300 characters on
  4 lines and `complete`, i.e. with no known secret or credential shape in it; control
  characters removed): `{v: 1, topic, tag, title (≤ 100), body (≤ 300; a finished turn: the first
  line of the answer, ≤ 120), level, ts, renotify, projectId?, open?: {kind, id, params},
  terminalId?, permissionId?, tool?, allow?}`. A permission request's body is `Needs your
  permission\n<tool>: <summary>`, or, when the request is whole and short enough to answer at a
  glance (`allow: true`: `complete` and at most 300 characters on 4 lines, the terminals'
  one-tap rule), `Needs your permission · <tool>\n<detail>`. The worker offers **Allow** /
  **Deny** only with `allow: true`, else **Review** (opens the session) / **Deny**, and never
  sends an allow for a payload without it.

**REST** (`/api/push/**`, platform). Only a device session can subscribe (the subscription is
bound to it); in-process callers (MCP) are refused on every write; every signed-in device may list,
change, test or remove any device's push (devices are fully trusted).

| route | body → answer |
|---|---|
| `GET /api/push` | `{publicKey, sessionId, subscriptions: SubInfo[]}`; `SubInfo {id, sessionId, device, service, createdAt, topics, quietWhenActive, lastOkAt, lastError, lastErrorAt, current, endpointHash}` (no endpoint path, no keys) |
| `POST /api/push/subscriptions` | `PushSubscription.toJSON()` + optional `topics`, `quietWhenActive` → `SubInfo` (400 for an endpoint off the allowlist or bad keys) |
| `PATCH /api/push/subscriptions/{id}` | `{topics?, quietWhenActive?}` → `SubInfo` |
| `DELETE /api/push/subscriptions/{id}` | `{ok}` |
| `POST /api/push/test` | `{subscriptionId?}` (default: this device) → `{results: [{subscriptionId, device, outcome: sent\|gone\|failed\|skipped\|superseded, status?, error?}]}`; 429 within 2 s of the last test |
| `POST /api/push/presence` | `{visible, active, tab?}` → `{ok}` |

Topics: `{attention, done, env, deploy, pipeline, notify}`, all on by default. **Event**
`push.changed {}` after any change of the subscription list (subscribe, change, remove, a service
answering 404/410, a session ending); every device's Settings refresh from it. `SendOutcome`
(`POST /api/platform/notify-test`, the `workbench_notify` tool) gained `push: queued | none | off |
full | rate-limited | n/a`.

**Config** `[push]` (`platform::push::PushConfig`, patchable from Settings): `subject` (a `mailto:`
address or an `https://` URL; anything else is refused on save) and `extra_endpoint_hosts`. Settings
saves edit config.toml in place (`platform::config_edit`, keeping comments and layout): the sections
it merges are the top-level keys of the old and new config as serialized, not a hand-kept list, so
`[push]` and the phase's `[lsp]` and `[debug]` keep the file's comments too. A save that cannot be
merged in place is written afresh and logged as a warning.

**The page** (`push.ts`; decisions in `pushLib.ts`, unit-tested). `pushSupport` explains what is
missing: HTTPS (`isSecureContext`; `http://localhost` counts), on iPhone/iPad the Home Screen app
(iOS 16.4+), or browser support. Turning push on asks for notification permission, subscribes with
the server's key (`userVisibleOnly`), stores `deviceKey` and `vapidKey` in IndexedDB and posts the
subscription; a local flag remembers the key it subscribed for. On every load `syncAction`
reconciles the browser and the server: re-register a rotated endpoint (`endpointHash`), subscribe
again for a new server key or a lost browser subscription, remove a record when notifications are
blocked, and turn push off here when it was removed from another device or dropped by the push
service. While push is on, the page's own in-tab notifications (hidden tab) are not shown, so
nothing arrives twice.

**UI.** Settings › Notifications: **Push** (why phones need HTTPS and iOS the Home Screen app;
this device: on/off, "Send test push", topics, "Hold pushes while I am using Workbench on another
device", last delivery or error; on the computer Workbench runs on, i.e. a loopback address as for
in-tab notifications, while the server shows desktop notifications: a note that push here would show
each one twice, with **Turn on anyway** in place of **Turn on push** for Allow / Deny in the
notification, and a warning while both are on), **Devices with push** (named after their device sessions, with
service, topics, last push, Test and Remove), the desktop and command settings, **Without push**
(in-tab notifications) and **Push service** (the VAPID contact). The phone's **More** tab has a
Notifications section (on/off, topics, test). The palette's "Notification settings" also answers to
push, phone and mobile. The settings page header now wraps its actions under the title when the
panel is narrow.

**Service install** (`workbench service`, `platform::service`, delegated from `main.rs`):
- `install [--enable] [--dry-run] [--name workbench]` writes
  `$XDG_CONFIG_HOME/systemd/user/<name>.service` (`ExecStart="<this binary, absolute>" serve`,
  `Restart=on-failure`, `RestartSec=5`, `KillMode=mixed` so Workbench stops its own process groups,
  `WorkingDirectory=%h`, `Environment=` for `PATH` (absolute entries, deduplicated) and the current
  `WORKBENCH_CONFIG_DIR`/`WORKBENCH_DATA_DIR`/`WORKBENCH_LOG`, quoted for systemd: `%%`, `$$` in
  `ExecStart`, `\"`), `$XDG_DATA_HOME/applications/<name>.desktop` (`Exec=[env WORKBENCH_…=…]
  <binary> open`, quoted per the Desktop Entry spec; `open` mints a one-time code and keeps the
  browser's session, see the Security model) and the SVG icon in
  `$XDG_DATA_HOME/icons/hicolor/scalable/apps/<name>.svg`; `XDG_*` fall back to `~/.config` and
  `~/.local/share`. Files carry a marker line; install refuses to overwrite a file without it.
  Without `--enable` nothing starts and the next steps are printed: `enable --now`, preceded by
  "stop the running Workbench (pid N) first" while one started by hand serves the same data dir, or
  `restart` when the unit already runs. `--enable` runs `systemctl --user daemon-reload` and
  `enable --now <name>.service` (for a unit that already runs: `enable`, then `restart`, so it loads
  the new binary and environment), and refuses while a Workbench started by hand serves the same data
  dir (the unit would fail to bind and restart forever).
- `uninstall [--dry-run]` runs `disable --now`, removes only marked files, then `daemon-reload`.
- `status` shows each file (installed, differs from this binary's, not ours), `is-enabled` /
  `is-active`, and whether a server runs for the data dir.
- **Windows** (`platform/service_windows.rs`, chosen by `cfg` in `platform/mod.rs`; registry,
  shortcut and detached starts in `util::os::autostart`): no Windows service (it would lose
  the desktop and Credential Manager and need admin), a per-user sign-in entry instead.
  `install` writes `%LOCALAPPDATA%\workbench\service.json` (`service-<name>.json`: a marker
  `note` and the set `WORKBENCH_CONFIG_DIR`/`DATA_DIR`/`LOG`; not `PATH`: whoever starts the
  supervisor, `install --enable` and `service open` included, starts it in the user's sign-in
  environment, `os::env::user_default`, with these) and the Start Menu
  shortcut `Workbench.lnk` (`Workbench-<name>.lnk`; written by the shell's ShellLink object,
  the marker in its description) running `workbenchw.exe [--name <name>] open`. `--enable`
  sets `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `Workbench`
  (`Workbench-<name>`) to `"<folder>\workbenchw.exe"` (an existing one of ours is always kept
  up to date) and starts the service now; it refuses while a server started by hand serves
  the data dir. Over a running service (on this data dir or the one of the settings being
  replaced) it hands the restart over: stopping the old server ends its terminals, and a
  command run in one with them, so it starts a new supervisor outside its own job
  (`service run --replace <old data dir>`, which stops the old service before it claims its
  event) and only waits; when its job keeps what it starts (a job without breakaway; Workbench's
  terminals allow it), it restarts nothing and says how. From a process elevated through UAC
  (`os::autostart::elevated`: the elevated half of a split token; the built-in Administrator
  and UAC off have no unelevated alternative and do not count) nothing is started or
  stopped, since the service and its agents would run as administrator; `service open`
  refuses to start one there too. Values and files not written by Workbench are refused.
  `workbenchw.exe` (GUI subsystem, no console) cannot use the server's modules (no library
  target), so it only starts `workbench.exe` from its own folder with `CREATE_NO_WINDOW`:
  `service run` (hidden), the supervisor, or `service open` (hidden; its error shown in a
  message box). The supervisor creates the data dir (so the event names hash its canonical
  path, as the server's do), then holds the event
  `<os::proc::stop_event_name(data_dir)>-service` (one per data dir; a second supervisor
  exits quietly), runs `workbench serve` in the saved environment with its output appended to
  `service.log` (moved to `.log.old` past 10 MB when a supervisor starts), restarts it 5 s
  after a non-zero exit, gives up after 5 failures within 60 s (a message box says so), and
  starts nothing while a server holds the data dir's stop event (`os::proc::server_running`:
  the server keeps it until its process ends, a graceful shutdown included; unlike
  runtime.json it cannot be stale). `service open` starts the supervisor when neither runs
  (apart from the caller: no inherited handles, outside its job when allowed), waits up to
  60 s for the server, then runs `workbench open` in the saved environment. `stop` sets both
  events and waits for the server's process to end and its port to close: up to 30 s, 40 s
  under the supervisor, which ends the server after 30 s. `status` reports the settings, the
  shortcut, the entry (on, differs, turned off in Task Manager: a `StartupApproved\Run` value
  with an odd first byte, or another program's) and the server (under the service or not);
  `uninstall` removes the entry, its `StartupApproved` value, the shortcut and the settings
  when they are ours, and stops a supervised server last. The events are `Local\` names,
  which each Windows session keeps apart. `Global\` events would reach across sessions with
  no privilege (`SeCreateGlobalPrivilege` is checked only for file mappings and symbolic
  links), but any account can create names there, and these are predictable (a hash of the
  data dir's path): another account could create one first and leave that server without a
  stop event. A server of the data dir in another session (started on the desktop while the
  command runs over SSH, whose processes run in session 0, or the other way round) is
  runtime.json's live pid in another session (`os::proc::session_of`) answering on its port.
  `status` names it, `stop` and `install --enable` refuse and say to manage it from its own
  session (or end it in Task Manager), `service open` opens it, and the supervisor leaves it
  alone. No message box is shown where nobody could answer it (`os::autostart::interactive`:
  session 0, or a window station that is not visible).

**Verified.**
- Rust unit and integration tests. RFC 8291 Appendix A gives exactly the RFC's intermediate values
  and 144-byte body (its example's "Content-Length: 145" is off by one). ES256 JWTs verify with
  `p256`, and a tampered claim, another audience or another key fail. The endpoint allowlist
  refuses look-alikes, nested WNS labels, IPs, ports, credentials, fragments and plain http;
  `extra_endpoint_hosts` wildcards work.
- End to end through the real router against a local mock push service, which checks the headers
  and the VAPID JWT and decrypts with the device's key: the test push; 429 with `Retry-After` then
  delivered; 5xx three times then recorded; 403 not retried; 410 dropping the subscription; presence,
  "in use elsewhere" and topics gating delivery; subscription validation (non-allowlisted endpoint,
  bad keys, a master-token caller, a cookie without the device key); a re-subscription keeping
  topics; a revoked device's subscription dropped and saved; `env.health` down reaching a device
  through the listener and queue; three notes with one tag coalescing into the newest; agent-note
  decisions on synthetic `AgentInfo.pendingPermission` values; payload limits.
- `workbench service` with scratch `XDG_*` and a fake `systemctl` first on `PATH`: dry run, refusal
  while a server runs, install `--enable` (`daemon-reload`, `enable --now`), `status`,
  `uninstall`; the printed hint while a server runs by hand; a re-install over a running unit
  (`restart`). The launcher passes `desktop-file-validate` and the unit `systemd-analyze --user
  verify`.
- Windows `workbench service`: `cfg(windows)` tests on scratch folders and a scratch
  `HKCU\Software\Workbench-test-<random>` key (install, dry run, refusals, `--enable`, the
  restart handover, Task Manager's off state, uninstall, status, names, stop and its wait for
  the supervisor's kill, and the supervisor's restarts, give-up, hands-off and stop), plus
  `util::os::autostart` (registry, a real shortcut, detached start, command-line quoting, the
  elevation type) and `os::proc::Event`. Type-checked for Windows only: none of it has run on Windows yet.
- Review fixes: config_edit keeps comments when only `[push]` changes (unit test, and the Settings
  PATCH replayed on an isolated instance); presence per tab (two tabs of one device, one hiding;
  the real browser sends a distinct `tab` per page and a closing tab reports only itself); no send
  slot held across a retry's wait (mock push service answering 503 with `Retry-After`); permission
  TTLs from `since` and the wait; the worker's answers to 2xx/409/404/400/429/503/network errors and
  401 (`sw.test.ts` runs `sw.js` against stand-ins for its globals; in headless Chrome, 409 and 503
  through a CDP mock and the real server's 404); the loopback note, "Turn on anyway" and the warning
  in Settings and the phone's More tab in both themes (the settings answer rewritten in the page to
  report desktop notifications, the instance itself keeping `[notify] desktop = false`).
- Headless Chrome against an isolated instance: the manifest parses with no errors and no
  installability errors, and the worker is active and controls the page. Turning push on (the
  browser's `PushManager` stubbed with a well-formed subscription on `push.invalid`, allowed through
  `extra_endpoint_hosts`, so nothing left the machine) records the device and hands the device key
  to IndexedDB. A test push to that host fails with the DNS cause shown. `ServiceWorker.
  deliverPushMessage` shows the notification with Allow / Deny and `requireInteraction`, and
  nothing while Workbench is visible. Allow POSTs `{id, decision: 'allow'}` with the
  device key (answered by a CDP `Fetch` mock of the terminals route) and is replaced by a silent
  "Allowed · Bash" that clears itself. Deny answered 409 says "No longer pending here". Against the real
  server the worker's cookie, origin and key pass the guard (404: the route belongs to the
  terminals area). A tap posts the target to an open window, which opens it, and `/?open=` opens
  it on load. Settings and the phone's More tab were screenshotted in both themes.

**Not verified / limits.** No real FCM, Mozilla, Apple or WNS delivery was attempted (tests never
POST to external services), nor a real `PushManager.subscribe` or a device lock screen. Clicking a
real notification with no window open (`clients.openWindow` needs the click's user activation) was
covered in two halves. Notification actions are unavailable on iOS, where a tap opens the session
instead. The worker's IndexedDB copy of the device key is as exposed as the page's `localStorage`
copy (same origin). Titles name the project and the session, and a lock screen shows them. The
computer Workbench runs on is recognised by a loopback page address, as for in-tab notifications
(a device session's `remote` flag cannot tell: behind `tailscale serve` or a local reverse proxy a
phone's session is not remote either), so a browser reaching Workbench through an SSH tunnel's
`localhost` is taken for it; **Turn on anyway** covers that.

**Contract notes.** Reads `AgentInfo.pending_permission` (with its `since`, `detail` and
`complete`) and `[agents] permission_wait` from the terminals' own types
(`terminals::{PendingPermission, permission_wait_secs}`; the build-time JSON probing was
replaced at integration) and posts to `POST /api/agents/{terminalId}/permission` exactly as
specified. A subagent's request is held for 30 s
only; its push still gets the configured wait as TTL, and the worker's 409 handling covers it. Additions
outside the area: `AuthState::sessions_ended()` (auth), `getDeviceKey()` in `api/client.ts`,
`Command::Service` in `main.rs`, the `sw.js`/manifest types in `spa.rs`, the manifest, icon and
theme-color tags in `index.html`, and the RustCrypto dependencies `p256` 0.14 (ecdh, ecdsa),
`aes-gcm` 0.11 and `hkdf` 0.12 in `Cargo.toml`. For the shared tables: REST prefix `/api/push/**`
(platform); event `push.changed`; CLI `workbench service install|uninstall|status` (and `stop`
on Windows); files
`data_dir/push/vapid.json` and `subscriptions.json`.

### Approvals and more agent CLIs (terminals)

`server/src/terminals/{permission,gemini}.rs`, `hooks.rs`, `agent.rs`, `providers.rs`;
`web/src/features/agents/{Permission.tsx,lib/permission.ts}`.

**Permission requests answered from Workbench** (Claude Code). Verified against Claude Code
2.1.283 (its bundle and a real session): for the main thread the permission dialog is shown
at once and `PermissionRequest` hooks run beside it; the first answer wins (a hook's answer
after the user's is ignored, and answering in the terminal does not cancel the pending
hook). Background subagents that may prompt await their hooks before showing the dialog.
So Workbench can hold the hook's HTTP response until a device answers:
- **Config** (`[agents]`): `answer_permissions = true` (default: safe, the terminal prompt stays
  usable) and `permission_wait = 600` (seconds, 30–3600). The session's settings give the
  `PermissionRequest` hook a timeout of the wait + 30 s (5 s when off: observed only). A request
  from a subagent (`agent_id` in the payload) is held at most 30 s. `AskUserQuestion` and
  `ExitPlanMode` (plan approval) are never answerable here: Claude Code 2.1.283 drops a hook's
  allow without `updatedInput` for tools that require the user's interaction
  (`rjo = new Set([ExitPlanMode, AskUserQuestion])` in its ask path); their attention texts
  say to answer in the terminal ("Claude asks you to approve its plan — answer in the
  terminal"). MCP tools flagged `anthropic/requiresUserInteraction` cannot be told from the
  payload: an answer Claude drops is caught on screen (below).
- **State.** `AgentInfo.pendingPermission: {id, tool, summary, since, sessionRule?, detail,
  complete} | null` is the first of the session's FIFO queue (Claude shows one dialog at a
  time; at most 16). The PermissionRequest payload has no `tool_use_id`: the open `PreToolUse`
  of the same tool and input names its call (this also fixes sessions that stayed "needs
  permission" until `Stop` after a prompt answered in the terminal).
  - `summary`: one line, masked, then cut at 140 characters. Never enough to approve on.
  - `detail` (added in review): the whole request as text on its real lines — the Bash command,
    the WebFetch URL, the WebSearch query, `File:` plus the Edit/MultiEdit old and new strings,
    the Write content, the NotebookEdit source, any other tool's input (MCP tools…) as pretty
    JSON — at most 16 KB (then cut with a `… (cut here: N more bytes)` line).
  - `sessionRule`: what "For session" allows, whole (every rule on one line, nothing dropped);
    a rule longer than 2000 characters is not offered at all (no For session).
  - `complete` (added in review): `detail` and `sessionRule` show the request whole — nothing
    cut and nothing masked. One-tap Allow is offered only then (below).
  - Masking of all three: the session's known secrets (`secrets::redact`) and credential
    patterns (Authorization/Bearer values, `*password*=`/`*token*=`, `--password`, URL userinfo,
    `ghp_`/`glpat-`/`sk-`/`xox?-`/`AKIA`/`AIza`/`wba_` tokens, private key bodies). A masked value
    never spans whitespace, quotes or shell syntax (`$(…)`, backticks, `${…}`, `|`, `;`, `&`,
    redirections, `\`), so what runs is never hidden behind a mask
    (`--author=$(curl${IFS}…|sh)` stays visible; `--author=` names no credential). Invisible
    characters (controls other than newline and tab, bidi overrides and isolates, zero-width
    characters, soft hyphens, BOMs) are shown as `⟨U+XXXX⟩` in all of them. The terminal itself
    shows the same command unmasked except for known secrets, so the detail reveals nothing
    the session's screen does not.
- **A request stops being pending** — its held response is then `{}` (no decision; Claude's own
  prompt decides) — when a device answers; its call ends (`PostToolUse`, `PostToolUseFailure`,
  `PermissionDenied`, or a transcript `tool_result`); the turn or session moves on
  (`UserPromptSubmit`, `Stop`, `StopFailure`, `SessionStart`, `SessionEnd`, a transcript
  interrupt, the process exits, a restart); **HEURISTIC:** Claude's dialog (`Do you want to
  proceed?`, `… make this edit to …`) was seen on the bottom 30 rows while the request was first
  and then is gone on two looks 500 ms apart (an allowed tool may run long before its
  `PostToolUse`); the wait runs out; or Claude drops the request (the handler's drop guard).
  The screen is looked at every 500 ms, once when the request arrives, and before every key
  that reaches the session (the terminal WebSocket's user input, `/keys`, `/input`, Workbench's
  own sends): answering in the terminal takes a key and the dialog is up at that moment, so a
  quick answer before the first periodic look is caught too (review fix; before it, such a
  request stayed answerable for the whole run of the allowed tool and a later device answer
  was reported as taken).
- **A device's answer is not the session going on** (review fix). When the answered request
  was the last one and its dialog is on screen, the session stays `needs_permission` with the
  attention "Answered from Workbench — waiting for Claude to go on" until the dialog is gone on
  two looks (then `working`), its call ends (`PostToolUse`…), Claude asks again, or the turn
  moves on. A dialog still up six looks (3 s) later means Claude did not take the answer (a
  tool that requires the user's interaction, or the terminal answered first): the attention
  becomes "Claude did not take the answer from Workbench — answer in the terminal" and an
  `agent.attention` event (without `permission`) says so once.
- **REST** `POST /api/agents/{terminalId}/permission {id, decision: 'allow'|'deny', message?,
  interrupt?, scope?: 'once'|'session'} → TerminalInfo`. Devices only: agent tokens get 401,
  in-process (MCP) calls and the master token 403. This stops agent tokens and Workbench's own
  MCP tools from answering; it is not a boundary against a process that can read
  `data_dir/token`: such a process can trade it for a device at `/auth` or type into Claude's
  dialog through `/input {force: true}`. The token file's 0600 mode and the agent's own
  permission prompts are what keep an agent from approving itself. `409 not_pending` once it
  is no longer pending, 400 for a bad body. A deny without `message` interrupts the turn, like
  "No" in the terminal; with one, Claude reads it and goes on. The message keeps its lines
  (control characters other than newline and tab become spaces, runs of blank lines one blank
  line, at most 2000 characters). `scope: 'session'` echoes
  Claude's own `permission_suggestions` as `updatedPermissions` moved to the in-memory `session`
  destination — only allow rules, `acceptEdits` and extra directories, never a mode that skips
  more prompts and never a settings file. `GET /api/agents/defaults` adds `answerPermissions`,
  `permissionWait` and `providers[].supports.answerPermissions`.
- **Events.** `agent.attention` gains `permission: PendingPermission | null`; every answerable
  request gets its own attention event (with its `permission`), also when it waits behind another.
- **UI.** Allow / For session (when Claude suggested a rule) / Deny / Deny with feedback… on the
  session's card (agents home), in the tab's header strip, on the phone's agents tab (rows and
  the full-screen terminal) and in a sticky toast (it goes away by itself once the request is
  settled anywhere). Buttons say that Claude's own prompt stays usable. Under the summary, the
  full request (`detail`, monospace, scrollable) whenever the summary does not contain it: open
  at first for commands and tool arguments, folded for file contents; a note when it is not
  `complete` ("Part of it is masked or cut here: read it in the terminal before allowing");
  and "For session allows `<rule>`" as visible text (a tooltip never shows on touch screens).
  The toast offers one-tap Allow only when the request is `complete` and at most 300
  characters on at most 4 lines, and then shows `detail` whole in a monospace block
  (`Toast.code`); otherwise it offers Deny and "Open to review". The toast uses the shell's
  `Toast.actions` and `Toast.code` (core). Composer help states the setting.
- **For platform push** (contract additions, both additive): a notification's Allow action
  follows the toast's rule — offered only when `pendingPermission.complete` is true and the
  notification shows the whole `detail` (at most 300 characters on 4 lines); otherwise Deny
  and Review (open the session). Done at integration: the push payload's `allow` and the
  service worker's actions (see "Integration").
- **Verified**: unit tests (queue, decisions, redaction, call correlation, and the hooks state
  machine: pending → answered from Workbench / in the terminal / timed out / session ended;
  review: request details and `complete`, masking that never hides shell syntax, rules shown
  whole, multi-line feedback, the awaited answer's `Closed` / `Ignored` looks, the input-time
  look, `ExitPlanMode` observed only),
  an end-to-end test through the real PTY, hook route and a device session against a fake Claude
  (`testdata/fake_cli.py` as `claude`: allow, deny, allow for the session, answered in the
  terminal with the late hook getting `{}`, answered in the terminal at once while the allowed
  tool runs 12 s,
  an allow Claude ignores (`sticky:`), a plan approval (`plan:`), observe-only mode, exit while
  pending, 401/403/409/400/404), headless screenshots of the card, tab strip, toasts and phone
  views in both themes (isolated instance, fake Claude), and a real
  Claude Code 2.1.283 (`--model haiku`, default permission mode, scratch project): the dialog is
  on screen while the hook is held; Allow clicked on the agents-home card → "Allowed by
  PermissionRequest hook", file created; Deny tapped on the phone view → "Interrupted · What
  should Claude do instead?", no file. Not verified with the real CLI: answering in the terminal
  (the third prompt raised no request: Haiku suggested `!` after the denial), subagent requests,
  and the edit dialog's marker (from the bundle, not seen live).

**Codex approvals** stay recognized on the screen (HEURISTIC, texts of codex-cli 0.157.1, the
current release) plus the rollout; the UI marks such states "recognized on the session's screen".
Codex's hooks were researched and not used: they are command or MCP-tool hooks only, every
non-managed hook needs per-hash trust (`hooks.state."<key>".trusted_hash` in config.toml, or
`--dangerously-bypass-hook-trust`, which would also run the repository's unreviewed hooks), its
`PermissionRequest` hooks are awaited *before* the approval prompt (a held hook would block the
TUI), and hook commands get a cleared environment. `ask_never_types_into_a_codex_approval_dialog`
is the fake-CLI end-to-end test.

**Kimi MCP: not done.** The Python `kimi-cli` (1.52.0) only prints that it is no longer
maintained; Kimi Code 2.1.1 has no MCP flag or variable and reads MCP servers only from
`$KIMI_CODE_HOME/mcp.json`, the git root's `.mcp.json` and `<cwd>/.kimi-code/mcp.json` (agent
files cannot declare servers, header values do not expand variables). Workbench writes none of
the user's or the repository's files, so Kimi sessions keep running without Workbench's MCP.

**New presets** (`ProviderKind::{Gemini, Aider}`; `claude`, `codex`, `kimi`, `gemini`, `aider` are
the presets now: `[agents.providers.aider]` configures the Aider preset, `kind = "custom"` keeps a
custom CLI of that name). Unavailable unless the command is found; nothing was run with an account.
- **Gemini CLI** (`@google/gemini-cli` 0.61.0, `--help` and bundled source): `gemini
  --session-id <uuid>` (the id is chosen at launch, like Claude's) / `--resume <uuid>` (a session
  Gemini has no file for starts again under its id), `--model`, presets `auto_edit`, `plan` and
  `yolo` (dangerous) through `--approval-mode`, `--include-directories` (the Workspace folders
  too); the initial prompt is pasted. State: output activity; its tool confirmation (`Allow once`,
  `Allow for this session`, `No, suggest changes (esc)`…) and folder trust prompt are recognized on
  screen. History reads `$GEMINI_CLI_HOME/.gemini` (else `~/.gemini`): `projects.json` maps the
  directory to a slug, sessions are `tmp/<slug>/chats/session-<time>-<id8>.jsonl` (older `.json`,
  `tmp/<sha256 of the path>/`); head and tail are read, bounded. No MCP: Gemini's settings are the
  only way, and a system settings file must be owned by root (a Workbench-written one is skipped).
- **Aider** (`aider-chat` 0.86.2 `--help`): `aider [--restore-chat-history] [--model] [--yes-always]`
  plus `args`; no session ids, no directories, no MCP; `yes-always` is the dangerous preset. Its
  `(Y)es/(N)o` confirmations are recognized on its last line. **Restarts** (review fix) restore
  the chat only when Aider wrote it for this session: `<git root>/.aider.chat.history.md` (else
  in the start directory) is noted (exists, size, mtime; `Record.aider_history` in meta.json)
  when the session first starts, and `--restore-chat-history` is passed on a restart or restore
  only when the file changed since and git does not track it (`git -c core.fsmonitor=false
  ls-files --error-unmatch`; when git cannot tell, it counts as tracked). A committed history is
  repository content: restoring it would put a hostile clone's fabricated conversation into the
  model's context. A repository's own `.aider.conf.yml` can still set `restore-chat-history`,
  as with Aider outside Workbench.
- Fake-CLI end-to-end tests: `gemini_sessions_start_under_their_id_ask_on_screen_and_resume`,
  `aider_sessions_confirm_on_screen_and_restore_their_chat` (`testdata/fake_cli.py` as
  `gemini` and `aider`).

New here: route `POST /api/agents/{id}/permission`; event field `agent.attention.permission`;
TS `AgentInfo.pendingPermission`, `PendingPermission` (+ `detail`, `complete` from the review),
`AgentProvider` + `'gemini' | 'aider'`, shell `Toast.actions`, `Toast.code`; config `[agents]
answer_permissions`, `permission_wait`. No new panels, tool windows, commands, shortcuts or MCP
tools.

### Local history, Markdown export, Workspace trash (files, workspace)

**Review Changes** (`history/AgentChangesPanel.tsx`, panel `agentChanges` `{projectId, terminalId, title?}`, id `agentChanges:<terminalId>`; opened from an agent session's context menu): what one Claude Code session changed, from Local History's agent attribution (`GET …/files/history/session?by=<terminal id>` → per file: the version before the session's first edit, `null` for a file it created (or one deleted before it), its first and last edits, the file's newest version, `changedSince` when someone else changed it after, `deleted`). Each file shows before → now (the editor buffer, editable) with Revert File (into the buffer, unsaved), Delete File for created ones, Restore for deleted ones; Revert All writes the earlier versions to disk only where the file is still the version Local History last saw (else a conflict is reported). Edits by shell commands and by other agent CLIs are not attributed, so they are not listed.

**Local History** (`server/src/files/history/`, `web/src/features/files/history/`), CLion's:
every version of a project text file Workbench saves and every change the files watcher
sees, per project in `data_dir/local-history/<pid>/`.
- **Store** (`store.rs`). `blobs/<h0h1>/<sha256>.zst`: each content once (zstd level 3,
  already in the dependency tree through tower-http), addressed by the sha256 of the raw
  bytes, which is the editor's etag of that version. `index.jsonl`: append-only lines
  `{id, t, p, k, h?, s, z, l?, by?, who?}`; a torn last line (a crash or a full disk
  mid-append) is skipped and cut off when the store opens (a complete line that only
  lacks its newline gets one), and a failed append truncates its own partial line, so
  the next line never lands on a fragment; `attr` lines (a late agent attribution)
  fold into the revision they name. Dedup: a version equal to
  the path's newest one is not recorded. Files and folders are 0600/0700.
- **What is recorded** (`mod.rs`), with its row label: `save` "Saved in Workbench" (the
  editor's `PUT …/files/write`, with the exact bytes written; "Replace in Files" for
  search/replace), and the version on disk before a file's first save (`base`);
  `disk` "Changed on disk" (a `fs.changed` batch, up to 300 paths, not one over the 500-path
  cap; after the watcher lost notifications, also the files modified since, see "Files
  watcher"; or a
  read whose content is not the newest version). A folder that appeared (created or
  moved in: the watcher reports only the folder, files may have landed before its
  watch) is walked for its files (gitignore-aware, no links, hard-ignored folders
  skipped) within the same 300-path budget; a folder holding more (a clone, an
  unpacked archive) is skipped whole. `agent` "External (agent) edit · <session
  title>" when a Claude Code `PostToolUse` hook of Write/Edit/MultiEdit/NotebookEdit names
  a file **of the session's own project** (a hook naming another project's file, or from
  a session without a project, changes nothing: the watcher still records the change as
  "Changed on disk"); a session in a dev container names container paths, mapped back
  through the workspace mount (`devcontainer::workspace_mount`, the `ExecTarget.map` of
  the terminal's container as the poller last saw it; not through the workspace folder,
  which is `/` for compose or may sit below the mount), and a path outside the mount is
  not attributed. If the watcher recorded that very content first (≤ 30 s), its revision
  becomes the agent's. Edits by
  an agent's shell commands carry no path and stay "Changed on disk" (no guessing).
  `base` "Opened in Workbench" (the first version the editor read) and "Last commit
  (HEAD)" (before the first recorded change of a git-tracked file with no history, its
  committed version from a bounded `git cat-file`; for a file with CRLFs on disk, with the
  line ends a checkout writes by git's rules on every OS (`core.autocrlf`, `core.eol`, the
  `text`/`eol`/`crlf` attributes, read with bounded `git config`/`git check-attr`);
  watcher batches of up to 20 files and hooks only). `deleted` (a tracked path, or
  everything tracked below a folder, is gone), `label` (Put Label…) and `auto` ("Before
  git pull": the first `git.op` line of any op but fetch, push and remote-branch deletion).
- **Never recorded:** sensitive paths (the slice's rules; the pruner also drops a path's
  history when it becomes sensitive, and no route serves one), `.git`, hard-ignored and
  gitignored paths, binary files, files over 2 MB, symlinks leaving the project.
- **Order and cost.** Snapshots of a project run one after the other (a per-project async
  lock), off the request path (saves, reads and hooks spawn them): disk reads first, then
  a short store lock to record. `files.history {paths}` after each.
- **Retention.** 7 days, 100 versions per file, 256 MB of compressed blobs per project
  (oldest first, down to 90%), 200 000 entries; the pruner runs a minute after start,
  hourly, and when a project goes over a cap; it rewrites the index, deletes unreferenced
  blobs, and removes the folder of a history that became empty (removed projects expire).
  Constants, not config.
- **REST** `/api/projects/{pid}/files/history`: `GET ?path=&limit=&before=` (a file's
  revisions and the labels on it, its folders or the project, newest first; `untracked`
  says why a file has none: `sensitive`, `git`, `ignored`, `binary`, `tooLarge`),
  `GET /dir?path=` (changes of the files below; `''` = Recent Changes; no `base`),
  `GET /revision?id=` (text, `lossy`, `previous`), `GET /diff?id=&against=previous|current|<id>&context=`
  (unified, 256 KB cap), `POST /label {path?, label}`, `GET /stats`.
- **Cross-slice.** `files::history::agent_hook(state, terminal_id, payload)` is called by
  the terminals hook route for every Claude hook payload (one line in
  `terminals/routes.rs`); `files::history::auto_label(state, pid, text)` is called by git
  before every operation that rewrites the working tree (see "Integration"). Added to
  devcontainer: `devcontainer::workspace_mount(state, pid, container_short_id) ->
  Option<(host dir, container dir)>` (no Docker call).
- **MCP** `files_local_history {path, limit?, revision?, diff?, against?, content?}`:
  read-only, the session's project only (`McpCtx::project_for`); a folder or `''` lists
  recent changes; a revision gives its unified diff or text; sensitive paths are refused.
- **UI.** Panel `localHistory` `{projectId, path, dir?, id?}`, id
  `localHistory:<pid>:<path>` (folders end in `/`, the project is `…:/`): versions grouped
  by day (time, kind icon, label, size) beside a Monaco diff; below 860 px the list sits
  above the diff. File history compares with the current buffer (the shared editor
  model: Revert to This Version lands in it, dirty and undoable; the diff's arrows revert
  one change; Ctrl+Alt+Z in the diff reverts the selected lines; Ctrl+S saves) or with
  the previous version; a deleted file offers Restore File. When the disk moved on
  under the buffer, the panel shows the editor's conflict choices above the diff
  (Reload from Disk / Keep Mine / Open in Editor; after a refused save Discard Mine /
  Overwrite), the header reads "Current (changed on disk)", and a refused save from the
  panel (button, Ctrl+S, the revert toast) says so in a toast. Folder history and Recent
  Changes show what each change did, with Show File History / Open File. Entry points:
  the tree menu and the editor's More menu ("Show Local History"), palette "Show Local
  History", "Recent Changes" (**Alt+Shift+C**), "Put Label…". Not on the phone.

**Export as HTML** (`web/src/features/files/export/`): the Markdown editor's More menu,
the preview panel and the palette ("Export Markdown as HTML…"). The file (the open
buffer, unsaved edits included) is rendered off screen by `ui/Markdown.tsx` itself
(sanitized; Mermaid strict), then: CSS rules for `.wb-prose`, `.wb-md-*`, `.hljs`,
`.wb-alert` copied from the app's stylesheets with the chosen theme's tokens resolved to
literal values (a document's own custom properties stay); relative images fetched
through the raw endpoint into data URIs (5 MB each, 25 MB in all; otherwise, and for
sensitive images, the relative link stays; repository Markdown is untrusted, so only
image files are fetched, named `.png/.jpg/.jpeg/.gif/.webp/.avif/.bmp/.ico/.svg` with no
`.git` segment, and embedded only when served as `image/*`: `![](../.git/config)` stays
a relative link, with the note "not an image, linked instead"); Mermaid redrawn in the export's theme when
it differs from the app's; copy buttons removed; `#anchor` links pointed at the ids the
renderer gave; a contents list from h1–h3 (a sidebar, above the text below 1180 px, gone
in print). The file carries a CSP (`default-src 'none'`, inline styles; images: data:,
the web, and `'self' file:` so relative images next to the file show when it is opened
from disk) and no script. Download, or Save next to the file (`<name>.html`, confirming
before it replaces one). Verified in headless Chrome in both themes and widths.

**Workspace trash** (`server/src/workspace/trash.rs`, `web/src/features/workspace/TrashView.tsx`).
Deleting a card always leaves an item `workspace-trash/<scope>/<folder>~<time>/` with the
card's files, `.card.json` (the registry entry) and `.trash.json` `{deletedAt, moved}`
(`moved: false`: the folder stayed with another card sharing it). Older items without
`.trash.json` count as moved, deleted at the time in their name. `DELETE …/cards/{id}`
now answers `{ok, trashItem}`. REST: `GET /api/workspace/{scope}/trash` (`all`: every
scope, plus scopes of removed projects, listed but not restorable), `POST
…/trash/{item}/restore` → the card (folder back under a free name if taken, never onto
anything: `RENAME_NOREPLACE`; entry appended with CAS under a free id; a registry failure
moves the folder back), `DELETE …/trash/{item}` (for good), `DELETE …/trash` (empty; `all`
too). Event `workspace.trash {scope}` (with the scope's `projectId`) after deletes,
restores and purges. UI: the home panel's Trash button (`workspace.home` param `view:
'trash'`), rows with Restore and Delete permanently (typed `delete`), Empty Trash (typed
`empty trash`), search; the Deleted toast has Undo; palette "Workspace Trash".

### Integration

The seven area branches were merged in order (lsp, debug, git, atlassian, platform,
terminals, files) onto the scaffold; the only textual conflict was `app.rs` (the lsp and
debug state fields). What the areas could not do alone:

- **Permission requests on the lock screen** (terminals ↔ platform). Push reads the
  terminals' typed `AgentInfo.pending_permission` and `[agents] permission_wait`
  (`terminals::{PendingPermission, permission_wait_secs}`) instead of probing JSON; a
  `terminal.updated` event's agent is deserialized as `AgentInfo`. A permission push offers
  Allow only under the toast's one-tap rule (`platform::push::one_tap_allow`: `complete`,
  at most 300 characters on 4 lines) and then shows the whole request; otherwise the body
  keeps the one-line summary and the notification offers Review and Deny. The service worker
  never sends an allow for a payload without `allow: true`. Tested end to end
  (`terminals::e2e_agents::permission_requests_reach_phones_as_pushes_that_answer_them`: the
  fake Claude's request reaches a mock push service, is decrypted with the phone's keys, and
  the notification's Allow — the worker's POST with the device key — answers it; a long
  command gets no Allow).
- **Local History labels before git rewrites the working tree** (git → files). The git
  routes call `files::history::auto_label` first ("Before git checkout topic", "Before git
  reset --hard HEAD", "Before rollback", "Before git stash", "Before git stash pop|apply",
  "Before shelve <name>", "Before unshelve", "Before git merge", "Before git rebase",
  "Before git cherry-pick", "Before git revert", "Before git bisect", "Before abort");
  pull and the interactive rebase run as `git.op` and are labelled by the files slice from
  their first event. A soft or mixed reset, staging and commits change no file and get
  none (`git::tests_flows::working_tree_rewrites_label_the_local_history_first`).
- **One model-URI parser.** lsp, debug and git read `file:///<projectId>/<path>` with the
  files contract's `modelFile` (debug's and git's own regexes are gone; behaviour kept: a
  project root is no file, git's action wants a project file).
- **Editor hooks side by side.** lsp, debug and git each add their actions a microtask after
  `onDidCreateEditor`, guarded against hooking an editor twice; action ids are distinct
  (`wb.lsp.*`, `wb.debug.*`, `wb.git.*`, the files slice's `wb.*`), and so are their context
  menu groups. Every key is in "Keyboard shortcuts": no two owners take the same key in the
  same focus. The one clash found was F7: the debugger took it in the capture phase during a
  live session, before the git diff viewer's Next Difference; widgets now declare the keys
  they handle (`data-wb-keys`) and the debugger leaves those to them.
- **The glyph margin is shared.** Breakpoints and the execution point (debug), Monaco's gutter
  lightbulb for lsp code actions, change bars (files, line decorations) and diagnostics show
  together; a click on the gutter lightbulb used to toggle a breakpoint as well as open the
  quick fixes, and now only opens them (verified in the browser: a breakpoint, a gutter
  lightbulb and change bars in one Rust editor).
- **Stripes and bars.** Bottom tool windows in order: Terminal, Problems, Find Usages, Git
  Log, Debug, Run, Activity; Find Usages has its own icon (the left stripe already has Find's
  magnifier). At 1280 px the top bar holds the project, the attention chip, a paused debug
  session's chip, the branch, the run configuration and Commands with room to spare; the
  status bar holds attention, code intelligence and its counts, and the branch. Narrow
  Settings panels (a split) stack each setting's control under its label.
- **`workbench open` and the launcher** (core finding from the platform review). No master
  token on a browser's command line, and a browser that already has a session keeps it: see
  the Security model (Auth).

Verified on an isolated instance built from the integration branch (port 7950, scratch
config and data, `[notify] desktop = false`, throwaway repositories): code intelligence
enabled from the editor banner in a Rust crate (rust-analyzer: diagnostics, Problems, hover,
inlay hints) and through the route in a TypeScript project (typescript-language-server:
diagnostics, Find Usages, Ctrl+B across files); a breakpoint set in the glyph margin of a C
file and a gdb session through the `[[debug]]` configuration's pre-launch build (stop,
execution point, step, resume to "Exited with code 0"); Local History recording an edit made
outside Workbench; line staging of one of two changed lines in the diff viewer and shelving
the rest (with its automatic label); a fake Claude session's pending permission on the agents
home answered with Allow from the page (desktop and phone); Settings › Notifications and the
phone's More tab; both themes; the bars at 1280 px.

### Help (help)

The user documentation, bundled with the app so it works offline and on a phone. `web/src/features/help/pages/*.md` are the pages: the file name orders them (`01-getting-started.md`), the part after the number is the slug (`getting-started`) and the first `# ` heading is the title. `pages.ts` loads them with `import.meta.glob(…?raw)`, so they ship in the web build and the binary; nothing is fetched. Pages link to each other by slug, `[text](agents)` or `[text](agents#heading-id)`: a relative link, which the Markdown renderer hands to `onLinkClick` (a `help:` scheme would be stripped by react-markdown's URL transform).

- **Panel `help`** (`HelpPanel.tsx`, id `help`, params `{page?}`): the page list with search on the left, the page rendered by `@/ui` Markdown on the right. Search (`searchPages`) needs every word in the page, ranks title hits first and shows a matching line. `HelpView` is shared with the phone (`compact`: the list and a page take turns).
- **Entry points:** F1 and the palette (*Help*, *Help: remote access and phone*, *Help: configuration*), a status bar item, and the phone's More tab. `openHelp(page?)` (`actions.ts`) opens or focuses the panel.
- **Guards:** `pages.test.ts` (unique slugs, a title each, every link resolves to a page, search finds the setup terms) and `render.test.ts` (every page renders through the real Markdown pipeline).
- **Adding a page:** add `NN-slug.md` starting with `# Title`; nothing else to register. Keep it true to the code: a setting named in a page must exist in `config/global.rs` or the Settings UI, and a shortcut in the table above.
