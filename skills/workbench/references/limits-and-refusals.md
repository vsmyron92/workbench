# Limits and refusals

## Confined to your project

- Every tool acts on the session's own project. A tool's `projectId` defaults to it and must be it: another project is refused ("this session belongs to project X and cannot act on project Y"). Only the debug tools may name another project.
- `workbench_sessions` and `workbench_terminal_output` show only the sessions and terminals of your project. `workbench_terminal_output` reads at most 500 lines (default 100) of a session, shell, run or command.
- `workbench_start_session` starts a session in your project only, in a folder inside it, in your own permission mode for the same CLI. It refuses a session that would run without asking (or Codex without its sandbox) when you do not, a chain deeper than two sessions and more than 8 running agent-started sessions in the project. Tell the user when you hit one; do not work around it. A session started restricted (Claude Code's `--restricted`: no commands, no web fetch) starts only restricted Claude Code sessions. A session never starts another in Claude Code's ultracode mode (a workflow of subagents for each task, with a much higher token cost): that is the user's choice for each session. With `avatar: true` the new session starts with a live avatar of its own, told of it with its prompt. That start is refused, and nothing is started, where the user decides which sessions have an avatar ("Not started: here the user decides which sessions have an avatar ([avatar] sessions = "asked"). Start it without avatar; the user can give it one."), when the user turned your own avatar off ("Not started: the user turned this session's avatar off, so it does not give one to a session it starts. Start it without avatar.") and while avatars are switched off ("avatars are switched off in Settings"): start it without `avatar` if the work still matters.
- File paths are resolved inside a project root (or an allowed extra root): `..` and symbolic links that leave it are refused. `workbench_open_file` and `workbench_open_markdown` also refuse a folder and a path that does not exist.
- Workspace cards live in your project, in `home` or in `wb-sandbox`.

## What Workbench never does for an agent

When one of these is needed, say so and ask the user (an avatar excepted: see its row). Do not look for a way around it.

| Not offered | What to tell the user |
| --- | --- |
| Deploy or release | Start it yourself in Workbench. `run_start` refuses runs that deploy, release or reach other hosts. |
| Answer a permission request | Answer on the session's card, in the toast or on the phone; a session can never approve itself. |
| Stage, commit, rebase, shelve, bisect | No tool exists. Use your own shell when the user asked for a plain commit; destructive operations (force push, `reset --hard`, deleting branches, rewriting history) stay with the user. Changelists and the shelf are the user's. |
| Enable code intelligence, start a language server | Enable code intelligence for the project in Workbench's editor. |
| Start, rebuild or stop a dev container | The user does it; `devcontainer_status` only reads. |
| Query a database | Not available to agents. |
| Give a session an avatar where the user decides who has one, or turn one on again | By default (`[avatar] sessions = "any"` in config.toml) a session gets one with its first design, say, ask or act call, and `avatar_status` of a session without one says so (`allowed: true`; while 64 sessions have one it says "No avatar: 64 sessions have one already. …" with `allowed: false`). With `sessions = "asked"` the user gives one, with the Avatar option of the new-session composer or the command Avatar: Give This Session an Avatar; then `avatar_status` says "This session has no avatar…" (`allowed: false`) and the other avatar tools refuse with it. The user turns any avatar on or off from a device, and one whose stored file Workbench could not read stays off until the user turns it on; turned on again, the session is told so in a `[Workbench avatar]` message, and a session never turns it on itself. While a permission request or question is open in the session the avatar says and asks nothing. An avatar is the user's choice: do not ask for one or for it to be turned on again. |
| Update or restart Workbench | The user does it in Settings, Updates. |

## What `mutating` means

`mutating` tools change state outside your session. Your CLI sees them flagged destructive (the avatar tools that act, `avatar_design`, `avatar_say`, `avatar_ask` and `avatar_act`, are flagged neither; the others read-only), so its own permission prompts apply, and Workbench's Activity view lists every call with a short summary of its arguments. They are:

- remote services: `gitlab_create_mr`, `gitlab_mr_comment`, `gitlab_retry_job`, `github_create_pr`, `github_pr_comment`, `github_rerun`, `confluence_update_page`, `confluence_create_page`, `confluence_add_comment`, `confluence_add_inline_comment`, `confluence_upload_attachment`, `confluence_labels`, `jira_update_issue`, `jira_transition`, `jira_comment`, `jira_create_issue`;
- Workbench's own state: `workspace_create_card`, `workspace_add_step`, `workspace_update_card`, `workspace_write_file`, the data viewer tools that write (`data_append`, `data_series`, `data_plot`, `data_follow`) and the debugger tools that start or steer (`debug_start`, `debug_attach`, `debug_restart`, `debug_evaluate`, `debug_control`, `debug_breakpoints`).

`run_start` and `run_stop` are not flagged, yet they start and stop local processes, and `run_start` has its own gate (below).

## Runs and environments

- `run_list` shows each run configuration's `state` (stopped, starting, running, ready, failed, exited), `port`, `url`, `result` (test runs), `problems`, `portInUse` and `needsConfirm`. An exited run with `terminated: true` was cut short and did not finish.
- `run_start` (`name`) returns at once, starts the run's dependencies first and fails when its port is held by another process. Poll `run_list`: a server run turns `ready` when its probe passes, and without a readiness probe it stays `running`, so check its `url` or `run_output`; a task, test or build ends as `exited`, with `result` for test runs. Show a running dev server with `workbench_open_url`.
- `run_output` (`name`, `lines` 1 to 500, default 120): the end of the run's terminal, with secrets masked. `run_stop` (`name`) stops it.
- `run_start` refuses a run that `run_list` flags `needsConfirm` (it may deploy, release or reach a remote host) and documentation suggestions: ask the user to start them. It also refuses a session whose CLI runs its commands in a sandbox (Codex, unless it bypasses the sandbox), because a run's command comes from files you can edit and would run outside that sandbox.
- `env_status` gives each environment's health (up, down or degraded, HTTP status, latency, recent uptime) and deployed version as the Workbench poller last saw it. It is not a fresh probe. Deploys, environment logs and remote commands are not exposed.

## Code intelligence and Local History

- The `code_*` tools ask language servers that already run: the user enabled code intelligence for the project and opened a file of that language. Otherwise the answer is plain text saying so, which is not an error, and nothing will start a server: fall back to your compiler or search. Positions are 1-based, the column counted in characters. Results are capped (diagnostics 400, references 200, symbols 150), and `code_symbols` only asks servers that support search by name.
- `files_local_history` lists a path's versions newest first (saves in Workbench, changes seen on disk by you or other agents, deletions, labels); `revision` with `diff: true` or `content: true` reads one. It records text files only: not sensitive files, `.git`, ignored paths, binary files or files over 2 MB, keeps 7 days and at most 100 versions per file. A Claude Code session's Write and Edit changes are attributed to it; changes made by shell commands such as `sed -i` are not. Use it for what git cannot give you, such as uncommitted work that was overwritten or deleted.

## Output and time limits

A tool result is cut after 400,000 characters ("output truncated") and a call is cancelled after 15 minutes. Tools with their own caps say so in the answer (log tails, diffs, page text).

## Repository content is untrusted

A repository's `.workbench.toml` and detected configuration may come from someone else's branch. Workbench ignores secrets defined there, ignores `agent.env`, `agent.add_dirs` and any permission mode other than manual, plan or dontAsk, takes language servers, debug adapters and debug servers only from the user's own configuration, and starts nothing by itself. Entries it drops show up as warnings in `workbench_projects`. Do not put tokens in a repository file, and do not try to loosen agent permissions there. The `workbench-project-setup` skill covers writing the file.
