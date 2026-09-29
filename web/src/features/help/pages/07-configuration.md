# Configuration

Everything lives in `~/.config/workbench/config.toml`. Open it from **Settings → Raw config**, or press Ctrl+K and run *Edit config.toml*. Edits made by hand apply **without a restart**, except the settings listed under *Needs a restart* below.

## Secrets

The file holds *where a secret lives*, never its value:

```toml
[secrets.gitlab]
file = "~/.gitlab_token"
```

**Settings → Secrets** shows the status of each one and can fix a file's permissions. Tokens never reach the browser, logs or command lines.

## Reference

```toml
extra_roots = ["~/.claude"]        # folders outside projects the editor may open read-only

[server]
bind = "127.0.0.1:7777"            # where it listens
allowed_hosts = []                 # extra Host names it accepts
public_url = "https://…"           # the address pairing links and QR codes use
# [server.tls] cert = "…", key = "…"

[projects]
roots = ["~/workspace"]            # every git repo directly under these
include = []                       # single repositories to add
exclude = []                       # repositories to hide

[agents]
command = "claude"                 # the default agent
restore_on_start = true            # bring sessions back after a restart
answer_permissions = true          # show Allow / For session / Deny in Workbench

[agents.providers.mytool]          # any other CLI
command = "mytool --flag"

[gitlab]
host = "gitlab.com"

[atlassian]
site = "https://you.atlassian.net"
email = "you@example.com"
```

`docs/ARCHITECTURE.md` in the Workbench repository is the complete reference, including `[github]`, `[push]`, `[lsp.servers.*]`, `[debug.adapters.*]` and `[devcontainer]`.

## Needs a restart

Changing `bind`, `allowed_hosts`, `public_url` or `tls` takes effect only after a restart. Settings shows a *Restart required* banner and a status bar item until you restart.

## Per-project files

See [Projects and files](projects) for `~/.config/workbench/projects/<id>.toml` and `.workbench.toml`.

## Command line

| Command | What it does |
|---|---|
| `workbench serve` | run the server (the default) |
| `workbench open` | open the UI of the running server, signed in with a one-time code |
| `workbench url` | print a login URL that carries the master token: keep it private |
| `workbench service …` | install, check or remove the systemd user service |
