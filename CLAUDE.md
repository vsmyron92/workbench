# CLAUDE.md

Workbench: a Rust (axum) server plus a React/TS SPA. It is an AI-centric workspace where agent sessions (Claude Code, Codex, Kimi Code, custom CLIs) sit next to the editor, git, GitLab, GitHub, Confluence/Jira, Workspace deliverable cards and deployed apps.

**Read `docs/ARCHITECTURE.md` before changing anything.** It defines slice ownership, the Rust and TS contracts, panel kinds, event names, REST prefixes and the security model.

## Rules

- **Build targets go under `~/.cache/workbench-targets/<name>`** (set `CARGO_TARGET_DIR`), never `/tmp`, which is a small RAM tmpfs.
- **Never run tests or dev servers against the owner's real Workbench state.** Always set `WORKBENCH_CONFIG_DIR` and `WORKBENCH_DATA_DIR` to scratch directories, and use a non-default port.
- **Stop servers by port** with `fuser -k PORT/tcp`. Never use `pkill -f`: the pattern matches your own shell.
- **The owner's repositories under `~/workspace` are read-only for development and tests.**
  - Do not commit, check out, stash or reset in them.
  - Git features are tested on throwaway repositories in scratch directories.
- **External services (GitLab, GitHub, Atlassian, production hosts) are read-only in tests.**
  - Only GET requests are allowed.
  - Write paths are tested against local mock servers.
  - Never ssh to production, never deploy, never create or modify remote pages or issues.
- **Secrets never reach the browser, argv, logs or test output.**
- **Repository content is untrusted.** Detected config and `.workbench.toml` never define secrets, never name config.toml's secrets, never loosen agents, and never make Workbench run a command by itself. See "Repository config is untrusted" in `docs/ARCHITECTURE.md`.
- **Every WebSocket handler selects on `state.auth.watch(caller).ended()`**, so signing a device out closes its sockets.
- **`npm run build` is the frontend type check.** Run it, `npm run lint` and `cargo test` before calling work done.
- **UI changes are verified in a real browser.** A headless Chrome screenshot is enough; see the harness path in your task.
- **Keep the design coherent:**
  - Use `@/ui` components and the tokens in `web/src/theme/tokens.css`.
  - Add no new colour literals in features.
  - Follow CLion conventions for VCS colours and layout.
