# Make it yours

## Change in Settings

Settings (Ctrl+,) saves into `~/.config/workbench/config.toml` (on Windows
`%APPDATA%\workbench\config.toml`) without disturbing its comments or layout. Edits you
make to the file by hand apply without a restart.

| Section | What it covers |
| --- | --- |
| General | Theme, editor font size, the keymap, how Markdown opens |
| Projects | Which folders are projects, and each project's configuration layers |
| Agents | Providers, the default model and effort, permission requests |
| Integrations | GitLab, GitHub or GitHub Enterprise, Atlassian |
| Secrets | Every secret reference, whether it resolves, token files others can read |
| Remote access, Notifications | Paired devices, push notifications |
| Raw config | The whole file, validated before it is saved |

## Change with an agent

| Ask for | Where it belongs |
| --- | --- |
| “Add a run configuration for the worker.” | `[[run]]` in the project's `.workbench.toml` |
| “Watch staging's health and let me deploy it.” | `[[env]]` in the machine overlay, `~/.config/workbench/projects/<id>.toml` (on Windows under `%APPDATA%\workbench`) |
| “Connect the dev database.” | Database › Add Data Source, with the *name* of a secret |
| “Add another agent CLI.” | `[agents.providers.<name>]` in `config.toml` |
| “Write up what you found.” | A Workspace card in the project |
| “Keep this snippet around.” | A scratch file (Ctrl+Alt+Shift+Insert) |

A useful instruction: “Read the project's `.workbench.toml` and Settings › Projects,
propose the smallest change, make it, and check that the run starts.”

## Three rules that do not bend

- **Config holds secret references, never values.** A reference names a file, an
  environment variable, a `.env` key, the keyring or a command. Values never reach the
  browser, a command line or a log.
- **A repository's `.workbench.toml` is untrusted.** It can be shared with your team,
  but it cannot define secrets, loosen an agent's permissions or make Workbench run
  anything by itself.
- **Only `config.toml` names programs Workbench may start:** agent CLIs, language
  servers, debug adapters, the dev container tools.

## Your own cards

Ask an agent for a card in any project; ask for one in Home when it belongs to no
project. You can also press **New card** in the Workspace and drop files onto it. The
**Hand work to an agent** example shows what to ask for, and a report template to copy.

When you are ready, archive these examples. You can bring back a deleted one from the
Workspace trash.
