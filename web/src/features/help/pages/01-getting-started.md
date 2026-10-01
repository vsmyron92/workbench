# Getting started

Workbench is a developer workspace in one browser window, on this computer or on your phone. Agent sessions (Claude Code, Codex, Kimi Code, Gemini CLI, Aider or any CLI you configure) sit at the centre. Around them are the editor, version control, GitLab and GitHub, Confluence and Jira, Workspace cards, run configurations and your deployed apps.

## The layout

- **Agents and terminals** are a window on the left. Its first tab, **Agents**, has the prompt for a new session and the project's sessions; every session, shell and run output is a tab after it, wrapping onto as many rows as needed, and **+** starts an agent session or a shell. Closing a tab stops what runs in it (an agent session can be resumed from the history). A shell under the editor instead comes from the **Terminal** tool window, the first button of the stripe's bottom group: its shells are tabs there, beside whatever the agents window shows.
- **Tool windows** are docked on the left, right and bottom of the workspace area, each opened from the icon stripe right of the agents column: Workspace first, then Files. Alt+1 opens Files, Alt+5 Debug, Alt+6 Problems, Alt+9 Git Log and Alt+0 Commit. **Settings** is the last button of that stripe.
- **The centre** holds panels as tabs: the Workspace cards, which a project opens on, editors, diffs, merge requests, cards and Settings. Each project keeps its own tabs.
- **The agents window** (left) has the project switcher on top and, at the right of that bar, how many sessions are working or need you (click it to go to the next one that waits). **The workspace window** beside it holds everything else, with the branch, run configurations, environments, CI and Commands in its top bar. Alt+F12, or the button at the top of the agents window, collapses the workspace window so the agents have the whole width. The **status bar** shows remote access.
- **The Workspace** (the first button of the stripe) starts with four example cards in Home: a welcome guide, a tour in screenshots, a report template for agents and a checklist for connecting your services. They stay until you archive them. The **Sandbox** tab is a playground for documents, reports and agent experiments; Reset in its toolbar empties it (nothing is lost for good: cards go to the trash).
- **On a phone** the same things are tabs along the bottom: Agents, Git, Workspace, Files, CI, Apps and More.

## The command palette

Press **Ctrl+K** (or Ctrl+Shift+P) to run any command by name. Press **Shift twice** for Search Everywhere: files, symbols, actions and text. Most commands show their shortcut. The keys follow CLion's keymap.

Press **F1** to open this Help. Type in the box at the top of Help to search every page.

## First start

The first start writes `~/.config/workbench/config.toml` from what it finds on this machine:

- **Projects:** every git repository directly under `~/workspace`.
- **GitLab:** from `~/.gitlab_token`.
- **GitHub:** from `~/.github_token`. Without one, public repositories still work, read-only.
- **Atlassian:** from `~/.atlassian_token`. Set `[atlassian] site` to your Atlassian address.

The config holds *references* to secrets, never the secrets themselves. See [Configuration](configuration).

**On Windows** (experimental) the config is `%APPDATA%\workbench\config.toml`, and `~` means your user folder (`%USERPROFILE%`), so projects come from `%USERPROFILE%\workspace` and tokens from files such as `%USERPROFILE%\.gitlab_token`. [Configuration](configuration) lists the other folders. The release archive's `install.ps1` installs Workbench, and updates it when you run a newer archive's: a running Workbench keeps its old files until you restart it. `install.ps1 -Uninstall` removes it again, once Workbench is stopped; your configuration and data stay. Programs you install while Workbench runs (Node.js, Python) are found by new terminals, runs and agent sessions right away; the Git features find Git for Windows once you restart Workbench: `workbench service stop`, then open Workbench from the Start Menu (its shortcut comes with `workbench service install`, see [Running Workbench as a service](service)) or run `workbench serve --open` in a new terminal. A program started from a Workbench terminal ends when that terminal is closed, restarted or killed, and so does a browser or editor it opens that was not running yet (an agent CLI's sign-in page, `code .`), every window of it: start your browser and editor outside Workbench first.

## Where next

- [Projects and files](projects)
- [Agents](agents)
- [Version control and CI](version-control)
- [Remote access, phone and notifications](remote-access)
- [Running Workbench as a service](service)
