# Agents

An **agent session** is a CLI such as Claude Code running in a terminal inside Workbench, in your project's directory. Workbench adds a card for it with status, and can answer its permission questions for you.

## Starting a session

Open the **Agents** tool window, or press **Ctrl+K** and run *New agent session*. Choose the provider and the project.

- **Claude Code** works without setup.
- **Codex**, **Kimi Code**, **Gemini CLI** and **Aider** are presets that appear once the program is installed.
- **Any other CLI:** add `[agents.providers.<name>]` with `command = "…"` to `config.toml`. See [Configuration](configuration).

Selected code in the editor can be sent to an agent with **Ctrl+Shift+A** (*Ask Agent About Selection*). A debugger stop can be handed over with *Ask agent about this stop*.

## Permission requests

When Claude Code asks for permission, Workbench shows the request on the session's card, on its tab, in a toast, and on the phone's Agents tab. Choose:

- **Allow** for this request only;
- **For session** to stop being asked about the same thing during this session;
- **Deny**.

Claude's own prompt in the terminal keeps working. **Whichever answer comes first wins.** Turn this off with `[agents] answer_permissions = false`.

## Remote Control

Claude Code's Remote Control can be switched on per session, or run as a server per project, from the Agents panel.

## Dev containers

A project with a `devcontainer.json` gets a **Dev container** chip in the top bar. Nothing is built or started until you approve the exact plan it shows, dangerous items first. Once the container runs, new shells, run configurations and (if you tick *Run in dev container*) agent sessions run inside it, while the files stay on this computer.
