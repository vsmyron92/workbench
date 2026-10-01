# Welcome to Workbench

Your agents write the code. Workbench keeps the whole project in view around them: the
editor, git, CI, docs and tickets, your running apps and the results your agents hand
back, in one window on this computer or on your phone.

This card is an example. It stays until you archive it: set its status to **Archived**
in the card's header. Archiving keeps its files.

## Start with one task

1. In the **Agents** column on the left, pick an agent and describe the result you
   want, or leave the prompt empty for an interactive session. It becomes a tab of that
   column and runs in the project's folder, in a real terminal.
2. Let it work. When a Claude Code session asks for permission, **Allow**, **For
   session** and **Deny** appear on its card, as a toast and on your phone. The prompt in
   the terminal keeps working too; the first answer wins.
3. Review what it changed. **Review Changes** in the session's menu lists every file it
   wrote, each against its version from before the session, with Revert per file or for
   all.
4. Ask for a deliverable when terminal text is not enough: “Compare the two approaches
   and put the findings in a Workspace card.” The card appears in the project's
   Workspace.
5. Give your decision back to the session: “Use approach B and keep the public API.”

## Where things are

| Where | What is there |
| --- | --- |
| Agents column | Agent sessions, shells and run output as tabs; **+** starts one |
| Left stripe | Workspace, Files, Commit, Find; Settings at its foot |
| Right stripe | GitLab, GitHub, Confluence, Jira, Apps, Database |
| Bottom stripe | Problems, TODO, Git Log, Debug, Run, Services |
| Top bars | The project switcher over the agents column; the branch switcher and run configurations over the workspace |
| Status bar | The language server, the branch, CI status, the dev container |

Double Shift searches files, symbols, text and actions. Ctrl+K lists every command.

## Workspace cards

A card is one piece of work with tabs: an HTML report, Markdown notes, a folder of
images, a PDF, a video or a 3D comparison. Each project has its own cards; **Home** holds
the ones that belong to no project, like these examples. **All** shows every card at
once.

- Pinned cards come first, then the most recently touched.
- A card nobody touched for seven days moves to the archive by itself. Pinned cards and
  examples stay until you archive them. Search looks through the archive too.
- Iterations of the same task are more tabs on one card (“v2 · tighter layout”), not new
  cards.
- Drop files from the project tree or your desktop onto a card to add them as tabs.
- Deleting a card moves it to the Workspace trash, where you can restore it.

Next: [Everyday use](everyday-use.md) · [Make it yours](make-it-yours.md)
