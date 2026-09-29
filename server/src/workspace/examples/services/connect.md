# Connect a service

Workbench never stores a token's value. Configuration holds a *reference* to where the
value already lives, and the server reads it only when it needs it.

## 1. Keep the token where it is

A token file readable only by you is the simplest place:

```bash
install -m 600 /dev/null ~/.gitlab_token
$EDITOR ~/.gitlab_token        # paste the token, save
```

On the first start Workbench finds `~/.gitlab_token`, `~/.github_token` and
`~/.atlassian_token` by itself. Settings › Secrets flags token files that others can read
and offers to fix them.

## 2. Name it in `[secrets]`

```toml
[secrets]
gitlab    = { file = "~/.gitlab_token" }
github    = { env = "GITHUB_TOKEN" }
atlassian = { keyring = "workbench/atlassian" }
db_url    = { dotenv = "app/.env", key = "DATABASE_URL" }
staging   = { command = ["pass", "show", "shop/staging"] }
```

## 3. Point the integration at the name

```toml
[gitlab]
host = "gitlab.com"            # or your own GitLab
token = "gitlab"

[atlassian]
site = "https://example.atlassian.net"
email = "you@example.com"
token = "atlassian"
```

Settings › Integrations writes the same thing for you.

## 4. Check with a read-only action

Open the service's window and list something: merge requests, a Confluence space, the
tables of a database. Then ask an agent to do the same through its tools. Only when both
work, move on to anything that writes.

## What never goes where

- A token's value never goes into `config.toml`, `.workbench.toml` or any file you commit.
- A repository's `.workbench.toml` cannot define secrets. The secret names it uses
  resolve only against your machine-local overlay, never your global tokens.
- Values never reach the browser, a command line or a log; terminal output that
  contains one is masked.
