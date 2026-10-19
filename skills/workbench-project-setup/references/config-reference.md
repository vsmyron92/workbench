# Project configuration reference

Checked against Workbench 0.16.0. Unknown keys and tables are ignored without any warning, so
spell every key exactly as written here. **req** marks a required key: a missing one, a wrong type
or an unknown enum value makes the whole file unreadable. `[[x]]` is a list of tables, `[x]` one table.
A top-level `schema` number is accepted and nothing reads it: leave it out.

## Layers, merging, reloading

Three layers, lowest first: detected (computed on every reload, never written to disk), the
repository's `.workbench.toml` (in the project root, `root` of `workbench_projects`), and the machine
overlay `~/.config/workbench/projects/<id>.toml` (`%APPDATA%\workbench\projects\<id>.toml` on Windows,
`$WORKBENCH_CONFIG_DIR/projects/` when that is set). `<id>` is the project's `id`: its folder name as a
slug, `shop`, then `shop-2` for a second folder of that name. Workbench itself writes the overlay (Settings >
Projects > Machine overlay: it validates, saves with mode 0600 and reloads the project): you show the user the TOML
for it and do not write the file.

| Entries | Merged by | A later layer |
| --- | --- | --- |
| `[[run]]`, `[[env]]`, `[[component]]`, `[[debug]]`, `[[database]]`, `[[agent.starter]]`, `[[links.url]]` | `name` | replaces the whole entry |
| `[[repository]]` | `path` (`./a//b/` is `a/b`) | like `[repo]`; `name` when set |
| `[repo]` | key | `gitlab`, `github`, `ci` each replaced whole when set; `remote` always (default `origin`: restate it) |
| `[hosts.NAME]`, `[secrets]`, `[toolchains]` | key | replaces that value |
| `[agent]` | key | scalars win when set, `env` merges by key, `add_dirs` unions |
| `[project]` | key | non-empty values win, `sensitive` unions |
| `[links.confluence]`, `[links.jira]` | section | replaced whole |

So an overlay `[[env]]` named like a committed one drops every key it does not repeat.
Layers load at start, on **Reload projects** (Ctrl+K), the Apps window's Re-detect button, **Save and reload**
in Settings > Projects, and when config.toml changes. Editing a project file on disk does nothing by itself.
A layer that does not parse is left out whole and a warning quotes line and column: for the overlay
that includes its `[secrets]`.

## What the repository layer may not do

`.workbench.toml` and detection are untrusted (a clone, someone's branch). Dropped, each with a project warning:

| Written in `.workbench.toml` | Result |
| --- | --- |
| `[secrets]` (any entry) | ignored: references are read only from the overlay and config.toml |
| `[agent] permission_mode` other than `manual`, `plan`, `dontAsk` | ignored |
| `[agent] env`, `[agent] add_dirs` | ignored |
| `[[run]] status` (a background command) | ignored unless the overlay has a run of that name |
| `[[env]]` with `health.via_host = true` whose `host` is not in the overlay's `[hosts]` | the whole `health` probe is dropped |
| `[links.confluence]` / `[links.jira]` without its own `token` | `email` cleared; `site` dropped unless it is `https://NAME.atlassian.net` or the configured Atlassian site |
| `[[repository]]` with a path outside the root, absolute, or not the top of a git repository | entry ignored |

Silently narrowed: secret *names* that repository entries use (`auth.password`, `[[database]]`
`password` and `url`, `token` of `[repo.*]`, `[[repository]]` and `[links.*]`, `${secret:NAME}` in `[[debug]] env`)
resolve only against the overlay's `[secrets]`, never config.toml's; a missing one fails when used, not at load.
`[repo.gitlab]` without `token` gets config.toml's GitLab token only for the configured host, scheme included.
`[lsp]` is not a project table (ignored without a warning); `[debug.adapters]` and `[debug.servers]` would make
`debug` a table where a project file has the list `[[debug]]`: a parse error, the whole file is left out.
On a click only (never by themselves): runs, `pre_launch`, `[debug.remote]` arguments and gdb commands, env
logs, commands and deploys. Plain HTTP `health` probes of `.workbench.toml` do run in the background.

## Values: placeholders and secrets

| Where | Expands |
| --- | --- |
| `[[run]]` `command`, `stop`, `status`, `preview`; `[env.deploy] command` | `{root}` (absolute project root), `{NAME}` for each `[toolchains]` entry; `{branch}`, `{sha}`, `{sha8}` in `command`, `preview`, env values and deploys |
| `[[run]]` `env`, `[[debug]]` `env` values | `${secret:NAME}`, then placeholders, then a leading `~/` |
| `[agent] env` values | `${secret:NAME}` and a leading `~/` only |
| `[debug.remote] server_args` | `{program}`, `{root}`, `${workspaceFolder}`, toolchains, ports `{port}` to `{port9}` |

`${VAR}`, `{{x}}` and `{a,b}` are left alone. Any other `{word}` is read as a placeholder and blocks a
run's start (`unknown placeholder`): bash `{1..3}` and `awk '{print}'` trip it. `{branch}` is refused when
the branch name holds shell metacharacters. Secrets expand only in env values, never in `command`, `url`
or `args`.

## [project]

`name` (display name, default the folder's), `sensitive` (list of gitignore-style globs of files Workbench does
not show, serve, search or hand to agent tools, such as `config/*.local.json`), `nested_repos` (bool, false =
list only `[[repository]]` entries). `id` and `root` are ignored; `tags` and `docs` are filled by detection.

## [repo] and [[repository]]

Normally unnecessary: GitLab and GitHub come from the `origin` remote when its host contains `gitlab` or
`github`, or is the host of config.toml's `[gitlab]` or `[github]`. Write them for other hosts.

```toml
[repo.gitlab]
host = "gitlab.example.com"     # host name (https assumed) or http(s):// URL; default gitlab.com
path = "acme/shop"              # namespace/project; with an empty path the section is ignored and the remote decides
token = "gitlab_shop"           # NAME of a [secrets] entry of the overlay; empty = config.toml's, same host only
```

`[repo]` keys: `remote` (git remote name, default `origin`), `gitlab` (`host`, `path`, `project_id` number,
`token`), `github` (`host` default `github.com` or an Enterprise host, `path` `owner/repo`, `token`).
`default_branch`, `[repo.ci]` and `gitlab.registry` are accepted but nothing reads them: leave them out.

A project may hold several git repositories. Workbench finds them up to three folders deep (not in
dependency or build folders, not linked worktrees); list them only to rename one or give it its own forge:

```toml
[project]
nested_repos = false            # optional: list only the entries below

[[repository]]
path = "services/api"           # req: relative to the project root, inside it, the top of a repository
name = "API"                    # switcher label, default the folder's name
[repository.gitlab]
host = "gitlab.example.com"
path = "acme/api"
token = "gitlab_api"

[[repository]]
path = "web"
[repository.github]
path = "acme/web"
```

A `[[repository]]` takes the keys of `[repo]` (`remote`, `gitlab`, `github`). A folder with a
`.workbench.toml` and no `.git` becomes a project of repositories (its first one is the default) when it sits
directly under a project root (default `~/workspace`) or was added in Settings > Projects. The git, GitLab
and GitHub tools take `repo` (an id from `workbench_projects`' `repositories`).

## [agent]

```toml
[agent]
model = "opus"
effort = "high"                 # low, medium, high, xhigh or max
permission_mode = "plan"        # in .workbench.toml only manual, plan or dontAsk survive

[[agent.starter]]               # one-click prompts in the new-session form
name = "Fix the failing test"
command = "Run the tests, fix the first failure and explain the cause."
```

Keys: `model`, `effort`, `permission_mode`, `remote_control` (bool: start Claude with `--remote-control`),
`add_dirs` (list, overlay only), `env` (table, overlay only). Project `model`, `effort` and `permission_mode`
apply to Claude Code sessions only. A value Workbench cannot accept (an `effort` outside the list, an unknown
`permission_mode`, a `model` with odd characters, an `add_dirs` folder that does not exist) makes Claude sessions
of the project refuse to start. Overlay `permission_mode` also takes `acceptEdits`, `auto`, `bypassPermissions`
(loosening is the user's to decide, in the overlay they paste). `add_dirs` are relative to the project root, absolute or `~/`.
A starter's prompt is its `command` key (`name` and `command` are both **req**).

## [links]

```toml
[links.confluence]
site = "https://acme.atlassian.net"
space = "SHOP"
root_pages = [123456]           # page ids of this project's tree (numbers)

[links.jira]
site = "https://acme.atlassian.net"
project_keys = ["SHOP"]
```

Confluence also takes `cloud_id`, `pinned` (table of title = page id), `archived`, `email`, `token`; Jira takes
`jql`, `email`, `token`. `token` names an overlay secret holding an API token or `email:token`; without it
config.toml's `[atlassian]` account is used. `[[links.url]]` (`name`, `url`) is accepted, nothing reads it.

## [hosts.NAME] (overlay)

```toml
[hosts.staging-box]
host = "203.0.113.10"           # req: letters, digits, . - : [ ]
user = "deploy"                 # default root
port = 2222                     # default 22
identity_file = "~/.ssh/shop_deploy"
```

An env's `host = "staging-box"` runs its logs, commands and deploys over `ssh -o BatchMode=yes` (no prompts:
key auth and a known host key must already work). No `host`, or `local` without such an entry, means this machine.

## [secrets] and [toolchains] (overlay)

```toml
[secrets]                       # names: letters, digits, _ . -  (up to 64)
db_password = { file = "~/.config/shop/db_password" }
api_token   = { env = "SHOP_API_TOKEN" }
deploy_key  = { keyring = "workbench/shop-deploy" }            # service/account
db_url      = { dotenv = { path = "~/shop/.env", key = "DATABASE_URL" } }
staging_pw  = { command = ["pass", "show", "shop/staging"] }  # argv; trimmed stdout

[toolchains]                    # name = path; use as {name} in run commands
node20 = "~/.nvm/versions/node/v20.11.0/bin/node"
```

`file` and `dotenv.path` take absolute or `~/` paths (a relative one is read from Workbench's own working
directory, not the project). The value must be non-empty. On Windows write paths as `~/x`, `C:/x` or `'C:\x'`:
in double quotes `\U` starts an escape and the whole overlay fails to parse.

## [[component]]

`name` **req**, `kind` **req** (free text: `cargo`, `cargo-workspace`, `npm`, `compose`, `python`, `go`, ...), `path`
(project-relative folder), `version`. Detection writes them. Workbench uses only `cargo` and
`cargo-workspace` (derived debug targets under `path`) and `compose`; write one only for that.

```toml
[[component]]
name = "api"
kind = "cargo"
path = "server"
```
