# Agent skills

A **skill** is a folder with a `SKILL.md`: a name, a description that says when to use it, and instructions an agent follows when it does. Claude Code, Codex, Gemini CLI and Kimi Code read the same format from different folders. The **Skills** tool window (right stripe, or **Ctrl+K** → *Show Agent Skills*) shows which ones each of your agents can use in the project, and what they cost.

## The window

The summary counts the skills your agents can **use**, those **off or hidden** (turned off in settings, from a plugin that is not enabled, or hidden by another skill of the same name; with several accounts, a skill counts as off or hidden only when no account has it) and those **to review** (click to filter). Each agent account has a row with its skills and what their descriptions cost **every session** in tokens; for Claude Code that is what its newest session here really received. Skills that pre-approve tools, register hooks or run shell commands are counted as able to **act without asking**.

The list is grouped: the project's, each account's own, plugins, claude.ai's, and **Built in**: Claude Code's own skills, known from the listing it recorded in its newest session here (a skill you removed after that session began is still in that listing until the next session starts). Right-click a row to copy its name or `/command`, or to open a project skill in the editor.

A row opens the skill's **panel**: what it is for, where it is, who reads it, how to start it, what it costs, what it can do on its own, what is wrong with it (a front matter that does not parse, a missing or very long description, a name that differs from its folder), its files and its text. Nothing in a skill is ever run by Workbench.

## Workbench's own skills

The **Workbench** group at the bottom offers optional skills that teach an agent to work well inside Workbench: choosing the right tool and the etiquette (`workbench`), handing work over as a Workspace card, filling the data viewer, driving the debugger, and writing a project's `.workbench.toml` (for the machine-only files it shows you what to paste and never writes them). Open one to read its files, then **Install** it for a Claude Code or Codex account: the files are copied into that account's own skills folder with a small marker, so Workbench can **Update** or **Remove** exactly what it installed and never touches a file you changed without asking. Nothing is installed unless you do it; an installed skill's description (about 130 tokens) is in every session of that account, so install the ones you use.

## Limits

Skills are read from disk each time (reload with the button). The skills Codex, Gemini CLI and Kimi Code bundle themselves are not listed, nor are accounts inside a dev container. Desktop only.

More in the repository's `docs/agent-skills.md`.
