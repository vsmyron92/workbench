# Working with agents

Workbench runs your agent CLIs as they are: real sessions in real terminals, with your
own accounts and your own settings. What it adds is everything around them.

## Start a session

Open **Agents** from the left stripe, or press Ctrl+Shift+A. Pick a provider, type a task
(or nothing, for an interactive session) and start. Options: a name, the model, the
effort, the permission mode, Claude Code's Remote Control, and, when the project has a
running dev container, **Run in dev container**.

You also start sessions from where the work is:

- **Ask Agent About Selection** in the editor (Ctrl+Shift+A with a selection).
- **Ask agent to fix** on a failed GitLab job, a failed test in a pipeline, or a failed
  GitHub job; the prompt carries the log or the test's output.
- **Ask agent about this stop** in the debugger.
- **Ask agent** on a Workspace card.

A prompt goes to the project's most recently active session that can take it, or starts
a new one. Sessions survive a Workbench restart, and Claude Code's history lets you resume
earlier conversations.

## Permission requests

When a Claude Code session asks to run a command or edit a file, the request appears on
its card, its tab, as a toast, and on your phone, with **Allow**, **For session** and
**Deny**. The prompt in the terminal keeps working too; whichever answer comes first wins.
Only signed-in devices can answer: a session can never approve itself through Workbench.
`[agents] answer_permissions = false` turns this off; `permission_wait` sets how long a
request waits.

## Review Changes

Workbench's Local History records every file a Claude Code session writes, attributed to
that session. **Review Changes** (a session's menu) lists them: each file's version from
before the session against the file now, with **Revert File**, **Delete File** for files
it created, **Restore File** for files it deleted, and **Revert All**. Changes made by
other people since are flagged before you revert.

## What agents can do in Workbench

Claude Code and Codex sessions get Workbench's MCP server, with tools grouped by what
they touch. Every tool is confined to the session's own project.

| Tools | What they are for |
| --- | --- |
| `workbench_*` | Open a file, a diff, Markdown or a URL in your window; notify you; list sessions and read their output; propose a commit message; read changelists |
| `gitlab_*`, `github_*` | Pipelines and runs, job logs, failed tests, merge and pull requests with their diffs; comments, retries and new MRs/PRs |
| `confluence_*`, `jira_*` | Search, read and write pages, comments, attachments and labels; issues, transitions and boards |
| `workspace_*` | Create Workspace cards and add their steps: the way an agent hands you a report |
| `run_*`, `env_status` | Start and read run configurations; environment health |
| `code_*`, `debug_state`, `files_local_history` | Diagnostics and symbols from running language servers; a paused debugger's state; Local History |
| `devcontainer_status` | Whether the dev container runs |

Tools never deploy, never run destructive git operations, never start a language server
or debugger, and never answer a permission request. Run configurations that deploy or
release are refused to agents; you start them.

## Workspace cards

A Workspace card is a deliverable with tabs: a report, a gallery of images, a PDF, a 3D
comparison, a Markdown note. Agents create them with the `workspace_*` tools; you see them
in the Workspace tool window, pinned or by freshness, and open them next to the code.

![A Workspace card: a release readiness report prepared by an agent](assets/workspace.png)

Reports are HTML files served sandboxed: they cannot read Workbench's cookies or storage,
reach the page around them, or call its API. Reports that link `../_shared/report.css`
and `../_shared/report.js` and set `data-wb-report="document"` on `<html>` get the
standard dark look and a click-to-zoom image lightbox. The **Hand work to an agent**
example in Home has prompts to try and a report template that uses all of it. A
repository with a `workspace/workspace.json` in Mr. Mak Workspace's format shows those
cards too.

## Other agent CLIs

Codex, Kimi Code, Gemini CLI and Aider are presets: install one and it appears in the
composer. Any other CLI is a `[agents.providers.<name>]` entry with its `command`. Codex
sessions get the MCP tools and their state from Codex's own session log; Kimi Code, Gemini
CLI, Aider and custom CLIs run without the MCP tools (their MCP settings live in files
Workbench does not change) and show activity from their output. Permission requests
answered from Workbench and Review Changes are Claude Code's.

## On Windows

Windows support is in progress ([windows-port.md](windows-port.md)); this is how agents and
terminals behave there.

- Terminals run PowerShell 7 (`pwsh`) when it is installed, else Windows PowerShell. Set
  `[terminals] shell` in `config.toml` for another shell, such as Git Bash's `bash.exe`.
- Claude Code's native `claude.exe` in `%USERPROFILE%\.local\bin` (where its installer puts
  it) is used before the npm package's `claude.cmd`. CLIs installed with npm (Codex, Gemini
  CLI and others) start as `node` and their script, never through cmd.exe.
- A CLI that is a batch file of its own (`.bat` or `.cmd`, not an npm shim) gets its first
  prompt pasted instead of passed on the command line when the prompt holds
  `% ! ^ & | < > "` or a line break, which cmd.exe would read as its own. It does not start
  when another of its arguments holds one.
- Killing or closing a terminal ends every process started in it, programs with windows
  included.
