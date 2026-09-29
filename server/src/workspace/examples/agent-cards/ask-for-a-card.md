# Hand work to an agent

Terminal scrollback is a poor place to keep an answer you will come back to. Ask for a
Workspace card instead: the agent writes the files, registers them as tabs, and the card
opens next to your code.

## Prompts to try

- “Profile the slow endpoint and put the findings in a Workspace card: what you measured,
  the three biggest costs, and what you would change first.”
- “Compare these two libraries for our use case. One card, a report with a table and your
  recommendation.”
- “Render the three layout options as screenshots into one card, one tab each.”
- “Add a v2 tab to the release card with today's test results. Keep v1.”
- “Write the migration plan as Markdown in a card, so I can edit it before we start.”

## What the agent does

Claude Code and Codex sessions get Workbench's `workspace_*` MCP tools:

| Tool | What it does |
| --- | --- |
| `workspace_list_cards` | Finds an existing card to add to, before creating another |
| `workspace_create_card` | Creates the card and returns its folder |
| `workspace_write_file` | Writes a file into that folder, for agents that cannot write there themselves |
| `workspace_add_step` | Registers a file or folder as a tab |
| `workspace_update_card` | Changes the status, pin, title, description or category |
| `workspace_open_card` | Opens the card in your window |

A session works in its own project's cards, or in Home. It never touches another
project's cards.

## What makes a good card

- **One task, one card.** Iterations become more tabs: “v1 · first pass”, “v2 · tighter
  layout”.
- **Self-contained reports.** Reports run sandboxed: relative files only, no CDN scripts,
  web fonts or remote images. Inline CSS and scripts are fine.
- **The shared look.** Link `../_shared/report.css` and `../_shared/report.js` and put
  `data-wb-report="document"` on `<html>`: the dark report style, and every image opens
  full size with arrow-key browsing and a download button.
- **Honest status.** Say what was checked and what still needs your decision; mark a card
  **Done** when it is, and archive it when it no longer matters.

The **Report template** tab is a starting point that uses every piece of the shared style.
Ask an agent to copy it: “Use the report template from the Home examples for this.”

## Cards in a repository

A project whose root holds `workspace/workspace.json`, in Mr. Mak Workspace's format,
shows those cards too, next to the ones Workbench keeps. They travel with the repository:
Workbench only changes their status and pin, and leaves their files to git.
