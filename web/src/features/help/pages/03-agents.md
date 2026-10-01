# Agents

An **agent session** is a CLI such as Claude Code running in a terminal inside Workbench, in your project's directory. Workbench adds a card for it with status, and can answer its permission questions for you.

## Starting a session

Use the **Agents** tab, the first tab of the agents column on the left, or press **Ctrl+K** and run *New agent session*. Choose the provider and the project. The session becomes a tab of that column, and **+** after the last tab starts another session or a shell.

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

Claude Code's Remote Control can be switched on per session, or run as a server per project, from the Agents tab of the column.

## Dev containers

A project with a `devcontainer.json` gets a **Dev container** chip in the top bar. Nothing is built or started until you approve the exact plan it shows, dangerous items first. Once the container runs, new shells, run configurations and (if you tick *Run in dev container*) agent sessions run inside it, while the files stay on this computer.

Workbench on Windows leaves dev containers out for now; the **Services** window (Alt+8) still lists Docker's containers there, marked *experimental*.

To use dev containers on a Windows computer, run the Linux Workbench inside a WSL 2 distribution, with Docker Engine installed in that same distribution. It is the Linux program there, so its projects are the distribution's own folders (`~/workspace` there), which Workbench on Windows refuses as `\\wsl$` paths. A Claude Code session in a container reaches Workbench through a listener on the container network's gateway, so that address has to be one of the distribution's. A port the container does not publish opens only from a browser inside the distribution, since its link is the container's own address; to open it from Windows, publish it (`forwardPorts` or `appPort` in `devcontainer.json`). With Docker Desktop's WSL integration the engine runs in Docker Desktop's own WSL distribution, apart from yours, so the gateway may not be one of your distribution's addresses: Workbench then refuses Claude Code sessions in the container and says why, and most likely cannot reach container addresses either, which it uses to see a run inside listen on its port. Neither setup has been tested yet.
