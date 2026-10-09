# Projects and files

A **project** is a folder, usually one git repository. Switch between them with the project switcher at the top of the agents window. Every tool window follows the current project.

## Adding and removing projects

Workbench lists each repository directly under the folders in `[projects] roots` (default `~/workspace`). Change that list in **Settings → Projects**, or add single repositories with `include` and hide some with `exclude`.

## Several repositories in one project

A project folder may hold more than one git repository: services cloned side by side, a monorepo with a submodule, a folder that is no repository itself. Workbench lists them all, and git and CI work on one at a time. The **repository switcher** in the top bar (it appears when a project has two or more) picks which one the Commit window, the Git Log, the branch widget, merge requests, pipelines and Actions show. The file tree and editor show the changes of every repository, and agents, terminals, runs and search work on the whole project.

Workbench finds repositories up to three folders below the project root (not in dependency or build folders, and not linked worktrees). List them yourself, or give one its own GitLab or GitHub project, with `[[repository]]` in `.workbench.toml`:

```toml
[[repository]]
path = "services/api"          # relative to the project root
name = "API"                   # shown in the switcher
[repository.gitlab]
host = "gitlab.example.com"
path = "acme/api"
token = "api_token"            # the name of a [secrets] entry in your machine file
```

It takes the same keys as `[repo]`. Put `nested_repos = false` under `[project]` to list only the entries you wrote. A folder with a `.workbench.toml` and no `.git` of its own is a project too, which is how a folder of repositories becomes one: its first repository is the default.

## What Workbench detects

From a project's files Workbench works out:

- **Run configurations:** Cargo, package.json, Python, Go, CMake, Gradle and Maven, Ruby, PHP, Elixir, Make, just, Taskfile, Procfile, compose files, Unity and .NET. They appear in the workspace window's top bar and in the Run tool window (Alt+4); a run's output is a tab of the agents column.
- **Environments:** from a Caddyfile and deploy scripts. They appear in the Apps tool window.

## Adjusting a project

There are two places, and they behave differently:

| File | Scope | What it can do |
|---|---|---|
| `~/.config/workbench/projects/<id>.toml` | this machine only | hosts, secrets, deploys, agent settings |
| `.workbench.toml` in the repository | committed with the code | run configurations and other non-sensitive settings |

On Windows the machine's file is `%APPDATA%\workbench\projects\<id>.toml`.

A repository's own config is **untrusted**. It can never define secrets, loosen agent permissions or make Workbench run a command by itself. That is deliberate: cloning a repository must not be able to take over your machine.

## Files and editing

- **Ctrl+P** opens Go to File. **Ctrl+Shift+F** searches inside files.
- **Ctrl+S** saves. Markdown files open rendered, with a Read / Split / Edit switch.
- **Local History** (Alt+Shift+C for recent changes) keeps every version Workbench saves, and every change it sees on disk, for a week. Edits made by agents are attributed to their session. You can diff and revert.

## Code intelligence and debugging

Language servers (rust-analyzer, typescript-language-server, pyright, gopls, clangd, Verible for Verilog and SystemVerilog, vhdl_ls for VHDL and others) give diagnostics, hover, completion and navigation. Install the ones you need; a missing one is named, with how to install it, above the first file it would serve. They run project code, so **nothing starts until you enable code intelligence for a project**: use the banner over its first source file, or the status bar item.

Debugging uses the Debug Adapter Protocol. Click the gutter to set a breakpoint, then press Shift+F9 to debug.

**Firmware on a microcontroller** is debugged through gdb and a debug server (OpenOCD, J-Link, pyOCD, `st-util`, or QEMU as a stand-in board). A `[[debug]]` entry in `.workbench.toml` with a `[debug.remote]` table starts the server, connects gdb, resets and downloads the program and runs it to `main`:

```toml
[[debug]]
name = "Blinky"
program = "build/blinky.elf"
pre_launch = "cmake --build build"
stop_on_entry = true

[debug.remote]
server = "openocd"
server_args = ["-f", "interface/stlink.cfg", "-f", "target/stm32f4x.cfg"]
```

The Debug window's **Start** view lists the GDBs (a GDB 14 or newer with Python, such as `gdb-multiarch`) and the debug servers it found, and a configuration's tooltip shows exactly what pressing Debug will run. The Console shows the server's own output, takes gdb commands (`monitor reset halt`) and *Registers* appear among the variables. Name the chip's SVD file (`svd`) to get a **Peripherals** tab with its registers by name, and `[[debug.remote.channels]]` to show its UART, RTT or SWO output in the Console. `docs/embedded-debugging.md` in the Workbench repository has the details.
