# Getting started

Workbench is a developer workspace in one browser window, on this computer or on your phone. Agent sessions (Claude Code, Codex, Kimi Code, Gemini CLI, Aider or any CLI you configure) sit at the centre. Around them are the editor, version control, GitLab and GitHub, Confluence and Jira, Workspace cards, run configurations and your deployed apps.

## The layout

- **Tool windows** are docked on the left, right and bottom, each opened from the icon stripe. Alt+1 opens Files, Alt+5 Debug, Alt+6 Problems, Alt+9 Git Log, Alt+0 Commit and Alt+F12 the Terminal.
- **The centre** holds panels as tabs: editors, agent terminals, diffs, merge requests, cards and Settings.
- **The top bar** has the project switcher. The **status bar** shows remote access and a Settings button.
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

## Where next

- [Projects and files](projects)
- [Agents](agents)
- [Version control and CI](version-control)
- [Remote access, phone and notifications](remote-access)
- [Running Workbench as a service](service)
