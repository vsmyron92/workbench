# Everyday use

## Code

**Files** (Alt+1) is the project tree with CLion's version control colours. The editor
uses CLion's keymap; Settings › General switches its editing keys to VS Code's. Once you
enable code intelligence for a project (its first source file offers it), language
servers add diagnostics, Go to Declaration and Find Usages. **Debug** (Shift+F9) runs
GDB, lldb-dap, CodeLLDB, debugpy or delve on a debug configuration.

Local History keeps every version Workbench saves and every change it sees on disk, so
nothing is lost between commits. **Recent Changes** is Alt+Shift+C.

## Version control

| Key | What it does |
| --- | --- |
| Alt+0 | Commit: changes, changelists, the shelf and stashes, line staging |
| Alt+9 | Git Log: branches, the graph and commit details |
| Ctrl+T | Update Project |
| Ctrl+Shift+K | Push |
| F7 / Shift+F7 | Next / previous difference in a diff |

## CI, reviews, docs and tickets

GitLab and GitHub, on the right stripe, show merge and pull requests, pipelines, Actions
runs, job logs and failed tests. **Ask agent to fix** on a failed job or test starts a
session with its log. Confluence pages open for reading and writing, with inline
comments; Jira boards move issues between columns.

## Things that run

Run configurations are detected from Cargo, package.json, Python, Go, CMake, compose
files and more. Start them from the top bar or **Run** (Alt+4). **Apps** shows your
environments with health checks and previews; a deploy always asks first. **Services**
(Alt+8) lists Docker containers and compose projects, and **Database** opens SQL consoles
on PostgreSQL.

## Agents at work

- Start a session from where the work is: **Ask Agent About Selection** (Ctrl+Shift+A in
  the editor), **Ask agent to fix** on CI, **Ask agent about this stop** in the debugger,
  **Ask agent** on a Workspace card.
- Sessions survive a Workbench restart, and Claude Code's history resumes earlier
  conversations.
- Agents reach Workbench through MCP tools confined to their own project. They open
  files and diffs in your window, read CI logs and Confluence pages, and hand you
  Workspace cards. They never deploy, run destructive git operations or answer
  permission requests.

## From your phone

Pair a phone in **Settings › Remote access** and add Workbench to its home screen. Push
notifications tell you when an agent needs you, with Allow and Deny on the notification
itself. Put TLS in front first: `tailscale serve`, a reverse proxy or `[server.tls]`.

## Keep the Workspace readable

- Give cards and sessions clear titles.
- Put one independent task on a card, and its revisions in tabs with readable names.
- Record what was checked and what still needs a decision.
- Archive finished work instead of letting it crowd the current list.

Next: [Make it yours](make-it-yours.md)
