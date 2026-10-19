# Runs and environments

Checked against Workbench 0.16.0. Read `config-reference.md` first for layers, merging and the trust rules.

## [[run]]: run configurations

```toml
[[run]]
name = "db"
kind = "service"                # daemonizes: the command exits 0, the service stays up
command = "docker compose up -d db"
stop = "docker compose stop db"
port = 5432

[[run]]
name = "api"
kind = "server"
command = "cargo run -p api"
cwd = "server"                  # relative to the project root, must exist
port = 8080
ready = { http = "http://127.0.0.1:8080/health", timeout_s = 60 }
env = { RUST_LOG = "info", DATABASE_URL = "${secret:db_url}" }
depends_on = ["db"]

[[run]]
name = "unit tests"
kind = "test"
command = "cargo test --workspace"
result_pattern = '^test result: \w+\. (?P<passed>\d+) passed; (?P<failed>\d+) failed'
```

| Key | Type, default | Meaning |
| --- | --- | --- |
| `name` **req** | string | unique; the merge key; also what `run_start` and `depends_on` name |
| `command` **req** | string | runs through `bash -lc` in a terminal (PowerShell on Windows); `{placeholders}` expand |
| `kind` | `task` (default), `server`, `test`, `build`, `service`, `editor` | how readiness and the end are judged, and which group lists it; `editor` is listed under Tools |
| `cwd` | string, `.` | project-relative (cannot leave the root), absolute or `~/`; must be a folder |
| `env` | table of strings | `${secret:NAME}`, placeholders and a leading `~/` expand; `""` sets an empty value |
| `port` | number | the port the command listens on (Workbench does not pass it on: put it in the command): used for readiness and conflicts, and as the link `http://localhost:PORT/` unless `preview` or a ready line gives another |
| `free_port` | bool, false | kills whatever listens on `port` before every start, agent starts included |
| `ready` | table | `log` (regex on ANSI-stripped output; a first capture group that is a URL becomes the run's URL), `http` (absolute URL polled every second; 2xx or 3xx passes), `timeout_s` (default 120) |
| `depends_on` | list | run names, or `port:N` (waits until N accepts connections, or starts the run that owns port N) |
| `preview` | string | the URL offered for the run (placeholders expand) |
| `stop`, `status` | string | `kind = "service"` only; `status` exit 0 means running; Workbench polls it in the background, so a `status` in `.workbench.toml` is dropped: such a run belongs in the overlay, which you show the user to paste |
| `result_pattern` | regex | named groups `passed` and `failed` give counts (summed over lines); a `status` group (with `name`, `detail`) gives one item per match; no groups marks each matching line failed |
| `group` | string | defaults from `kind` (`dev`, `test`, `build`, `tasks`, `tools`); `deploy` marks a run that needs the user's click |
| `source` | string | provenance label: leave it out |

Behaviour worth knowing:

- A server is `ready` when every `ready` probe passes; with no probe but a `port`, when the port accepts
  connections; with neither it stays `running`. A probe that never passes only raises a warning after
  `timeout_s`: the run keeps going. `test`, `build` and `task` end `exited` on exit code 0 and `failed` otherwise.
- A service is `ready` when its start command exits 0 and `status` exits 0 (overlay), else when `port` accepts
  connections, else at once.
- A taken `port` fails a start with `port_in_use` unless `free_port = true` (or the user agrees in the UI).
- A run whose name or command deploys, releases, publishes, reaches a remote host (`ssh`, `rsync`, `kubectl`,
  `docker push`, `git push`, `sudo`, `kill`, `rm -rf`, `curl -X POST`...), or mentions `token`, `secret`, `passw`,
  `api_key` or `credential`, or sits in group `deploy`, has `needsConfirm`: the UI asks first and `run_start`
  refuses it (it also refuses every run for a sandboxed agent such as Codex). Leave those to the user.
- Restating the name of a detected run replaces it whole (`group`, `ready`, `env` and the rest are lost).

## [[env]]: environments

```toml
[[env]]
name = "staging"
kind = "staging"                # production, staging, preview or development (default)
url = "https://staging.example.com"
health = { url = "https://staging.example.com/api/health", json_pointer = "/status", equals = "ok", interval_s = 60 }
version = { http = "https://staging.example.com/api/version", json_pointer = "/sha" }
```

An environment with a host, basic auth or a deploy goes whole into the overlay, as TOML you show the user to paste
(it replaces a committed entry of the same name entirely, so keep one definition, not two halves):

```toml
# ~/.config/workbench/projects/shop.toml
[hosts.staging-box]
host = "203.0.113.10"
user = "deploy"

[secrets]
staging_pw = { file = "~/.config/shop/staging_pw" }

[[env]]
name = "staging"
kind = "staging"
url = "https://staging.example.com"
host = "staging-box"
health = { url = "https://staging.example.com/api/health" }
auth = { user = "ada", password = "staging_pw", except = ["/api/*"] }
logs = [{ name = "api", command = "docker logs -f --tail 300 shop-api-1" }]
deploy = { command = "./deploy.sh {sha8}", confirm = "typed", require_green_pipeline = true, only_ref = "main" }
```

| Key | Meaning |
| --- | --- |
| `name` **req**, `url` **req** | `url` is what the preview shows; keep it non-empty |
| `kind` | `production`, `staging`, `preview`, `development` |
| `host` | a `[hosts.NAME]` key: logs, commands, version command and deploys run there over ssh; none means this machine |
| `health` | a table (not a string): `url` **req**, `expect_status` (200), `json_pointer` (RFC 6901, such as `/status`) with `equals` (the value compared as text; without `equals` it must be truthy), `interval_s` (60, clamped 10 to 86400), `timeout_ms` (5000, clamped 200 to 60000), `via_host` (probe with curl on the host: needs the host in the overlay) |
| `version` | table: `http` (probed with each health check, with `json_pointer`), `command` (run over ssh on demand, after a deploy), `pattern` (regex, group `sha`) |
| `auth` | table: `user` **req**, `password` **req** (a secret name), `except` (path patterns `/api/*`, `*.json`, `*` or exact, sent without auth) |
| `logs` | list of `{ name, command }`, or `[[env.logs]]`: followed in a terminal on a click |
| `command` | `[[env.command]]` entries `name`, `command`, `confirm` (bool): extra commands on a click |
| `deploy` | table: `command` **req** (`{sha}`, `{sha8}`, `{branch}`), `local` (bool: run here instead of on `host`), `confirm` (`none`, `click` default, `typed`), `require_green_pipeline` (the commit's CI must be `success`), `only_ref` (the checked-out branch must be this one), `after` (another env that must already run this commit) |

- A probe is an HTTP GET from Workbench: it starts within seconds of a reload, then repeats every `interval_s`
  (a failure is retried once after 2 s). Point it at a cheap health endpoint. `env_status` shows what it found;
  an env without `health` is never checked and reads `unknown`.
- A deploy never starts from an agent: there is no tool for it. Do not run a `deploy` command in a shell
  either. Keep `confirm` at `click` or `typed` (production: `typed`).
- `name` and `command` are both **req** in `logs` and `[[env.command]]` entries.
