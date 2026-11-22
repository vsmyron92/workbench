# Workbench's optional agent skills

Skills that teach an AI coding agent to work well inside [Workbench](../README.md): which of
its tools to use for what, handing work over as a Workspace card, filling the data viewer,
driving the debugger, writing a project's `.workbench.toml`, and giving a session's live avatar
a look and a voice.

| Skill | For |
| --- | --- |
| [`workbench`](workbench/SKILL.md) | choosing the right tool, CI and review triage, etiquette, what Workbench never does for an agent |
| [`workbench-report-card`](workbench-report-card/SKILL.md) | reports, comparisons and galleries as Workspace cards |
| [`workbench-data-capture`](workbench-data-capture/SKILL.md) | measurements into the live data viewer |
| [`workbench-debug-session`](workbench-debug-session/SKILL.md) | the debugger, for native programs and firmware |
| [`workbench-project-setup`](workbench-project-setup/SKILL.md) | the `.workbench.toml` schema |
| [`workbench-avatar`](workbench-avatar/SKILL.md) | a session's live avatar: its design, what it says and asks |

They are embedded in the `workbench` program and **never installed unless you ask**: open the
Skills window, read a skill, press **Install** for a Claude Code or Codex account
([agent skills](../docs/agent-skills.md)). To use one without Workbench, copy its folder to where
your agent reads skills (`~/.claude/skills/`, `$CODEX_HOME/skills/`). Each folder is a standard
[Agent Skill](https://agentskills.io): a `SKILL.md` and `references/` it links to, text only. They
are of use in a session started from Workbench: most need its MCP tools (Claude Code or Codex),
the avatar skill also works through the command or the curl calls its brief names.

A test keeps them honest: every tool a skill names must exist, every link must resolve, every TOML
example must parse as the file it says it is, every design in the avatar skill must pass the
validator without a warning, and the front matter must stay portable.
