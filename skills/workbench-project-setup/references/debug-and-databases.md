# Debug launch configurations and data sources

Checked against Workbench 0.16.0. Read `config-reference.md` first for layers, merging and the trust rules.

## [[debug]]: launch configurations

Cargo targets, CMake executables, Python run configurations and Go main packages already appear in the Debug
window's Start view without any entry. Write `[[debug]]` for arguments, environment, a build step, an attach or a
board. An explicit entry wins over a derived one of the same name.

```toml
[[debug]]
name = "import job"
program = "scripts/import.py"
args = ["--dry-run"]
env = { DATABASE_URL = "${secret:db_url}" }

[[debug]]
name = "firmware (OpenOCD)"     # gdb through a debug server, on a board
program = "build/firmware.elf"
pre_launch = "cmake --build build"
stop_on_entry = true            # stop at main

[debug.remote]
server = "openocd"
server_args = ["-f", "interface/stlink.cfg", "-f", "target/stm32f4x.cfg"]
```

`[debug]` is a list in a project file (`[[debug]]`); `[debug.remote]` after it belongs to the last entry.

| Key | Meaning |
| --- | --- |
| `name` **req** | merge key, shown in the Start view |
| `adapter` | an adapter id: a preset (`gdb`, `lldb-dap`, `codelldb`, `debugpy`, `delve`, `gdb-multiarch`, `arm-none-eabi-gdb`, `riscv-gdb`, `xtensa-gdb`) or one config.toml defines; never a command. Default: the adapter for `language` |
| `language` | `c`, `cpp`, `rust`, `python`, `go`, ...; guessed when absent (a `.py` file or `module` means python, `.go` or a go.mod package means go, a Cargo.toml means rust, else cpp) |
| `request` | `launch` (default) or `attach` |
| `program` | the executable or script: relative to the **project root** (not to `cwd`), absolute or `~/`; `{root}`, `${workspaceFolder}` and toolchains expand |
| `module` | Python: a module to run (`python -m`) instead of `program` |
| `args` | list of strings |
| `cwd` | folder, project-relative (inside the root), absolute or `~/`; default the root; `pre_launch` runs there too |
| `env` | table; `${secret:NAME}` expands at launch and, from `.workbench.toml`, resolves only against the overlay's `[secrets]` |
| `pre_launch` | a run configuration's name, or a command run in a terminal first; a failure stops the launch; a run with `needsConfirm` is refused |
| `stop_on_entry` | bool: stop at the program's entry (`main`) |
| `console` | `terminal` (default: the program gets a Workbench terminal) or `console` (output in the debug console) |
| `pid` | attach: the process id; without one the UI asks |
| `extra` | table merged last into the adapter's launch or attach arguments (adapter specific) |
| `remote` | `[debug.remote]`: gdb connects to a stub on a board or another machine instead of starting `program` |

Starting a configuration runs its `pre_launch`, the debug server and gdb on the user's machine, outside the
sandbox an agent CLI such as Codex keeps, and can flash a chip. Do not start one with `debug_start` to test the
file unless the user asked. Ask the user to open the Debug tool window (bottom stripe): its Start view marks a
configuration that has problems with a warning triangle and, for a board, shows the exact server command line.

### [debug.remote]

| Key | Meaning |
| --- | --- |
| `server` | the debug server to start: `openocd`, `jlink`, `pyocd`, `st-util`, `qemu-arm` or an id from config.toml's `[debug.servers]`; omit when a stub already runs and `connect` says where |
| `server_args` | list, after the server's own arguments: probe, chip, board. `{program}`, `{root}`, `${workspaceFolder}`, toolchains, `{port}` to `{port9}` expand |
| `connect` | gdb's `target remote` argument, `host:port` or a serial device; default the server's port here |
| `port` | the stub's port (default a free one per session) |
| `init`, `reset` | gdb commands, one string or a list; the server's defaults apply when absent, `reset = []` means none |
| `download` | bool: `load` the program to the target; default yes for servers with flash, no for QEMU |
| `source_map` | list of `[from, to]` pairs (`to` takes `{root}`): gdb's `set substitute-path` |
| `svd` | CMSIS-SVD file (project-relative, absolute or `~/`): adds the Peripherals tab |
| `channels` | `[[debug.remote.channels]]`: `port` **req** (number, or `"{port2}"` to `"{port9}"` that the server's arguments also use), `name`, `format` (`text` or `itm`), `itm_port`; at most 8 |
| `extended`, `attach`, `exec_file` | `target extended-remote` (`gdbserver --multi`, Black Magic Probe); `attach` is a process id or the probe's target number |
| `on_stop` | `resume` (default) or `halt`: what the target does when the session stops |
| `stop_at` | `main`, `reset` or a gdb location (`app_main`, `file.c:42`); naming a place stops there without `stop_on_entry` |

```toml
[[debug]]
name = "board (stub already running)"
program = "build/app"

[debug.remote]
connect = "board.example.com:2345"
download = false
```

A debug server, an adapter or a language server is a command Workbench starts, so it comes only from
config.toml. Do not edit config.toml: show the user the snippet and let them paste it (Settings > Raw config).
Fields of `[debug.servers.ID]`: `command`, `args` (with `{port}`, `{program}`), `label`, `env`, `enabled`, `init`,
`reset`, `download`, `ready_timeout_s`, `live_port`, `install_hint`; of `[debug.adapters.ID]`: `kind` (`gdb`, `lldb`,
`codelldb`, `debugpy`, `delve`, `generic`), `command`, `args`, `languages`, `transport` (`stdio`, `tcp`), `label`,
`enabled`, `env`, `adapter_id`, `launch_defaults`, `connect_timeout_s`, `install_hint`.

```toml
# config.toml (the user's file)
[debug.servers.openocd]
command = "~/xpacks/openocd/bin/openocd"

[debug.adapters.my-gdb]
kind = "gdb"
command = "~/tools/riscv/bin/riscv32-unknown-elf-gdb"
args = ["-q", "-i", "dap"]

[debug.default_adapter]
embedded = "my-gdb"             # the GDB for every remote target; or rust = "codelldb"
```

## [[database]]: data sources

PostgreSQL only. The Database window (right stripe) lists them; the user queries, agents cannot (no tool, the
routes refuse agents).

```toml
[[database]]
name = "dev"
host = "localhost"
port = 5432
database = "shop"
user = "shop"
password = "shop_db_password"   # the NAME of a [secrets] entry in the overlay, never the password
sslmode = "prefer"
read_only = true
```

| Key | Meaning |
| --- | --- |
| `name` **req** | 1 to 64 characters: letters, digits, spaces, `.`, `_`, `-`, starting with a letter or digit (it appears in URLs) |
| `kind` | `postgres`, the only kind |
| `host`, `port`, `database`, `user` | defaults: `localhost`, 5432, the database named like the user, the OS user |
| `password` | secret name of the password |
| `url` | secret name of a whole connection URL (`postgres://...` or `key=value ...`, such as a `.env`'s `DATABASE_URL`); the fields above override its parts |
| `sslmode` | `disable`, `prefer` (default), `require` (encrypted, not verified), `verify-full` (verified) |
| `read_only` | bool: sessions start read-only: a guard against slips, not a permission |

Without `password` or `url`, `~/.pgpass` is used as psql would (the file must not be readable by others). Secret
names used from `.workbench.toml` resolve only against the overlay's `[secrets]`; the overlay entry of the same
name replaces the committed one whole, and that is what the Database window's own editor writes.
Ask the user to right-click the source > Test Connection. A password typed into the browser is refused.
