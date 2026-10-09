# Changelog

## 0.12.3 - 2026-10-08

- **3D comparisons with many models show every model whole.** A test with more models than fit in one
  row squeezed its rows until each pane clipped its model's stage, which was still framed at full height,
  so only the tops of the models showed. Rows now keep the height a pane needs (a little less on short
  screens) and the panes scroll. While they scroll, the wheel scrolls them and Ctrl/⌘ + wheel (or a pinch)
  zooms the model under the pointer; on a touch screen a vertical swipe scrolls without tilting the models,
  and a sideways one turns them. A note over the viewer says so. A single row still fits the viewer,
  however short, and a few pixels out of view do not change what the wheel does.
- **A 3D test that lists the same model file twice** no longer leaves panes of the previous test behind
  when you switch tests.
- **Closing a 3D pane frees its WebGL context at once.** Switching between tests with many models used to
  leave old contexts alive until the browser dropped the oldest ("Too many active WebGL contexts").

## 0.12.2 - 2026-10-08

- **The "N working" pill opens a list of sessions.** Clicking it in the top bar used to go to the Agents
  tab. It now drops down the working sessions; picking one opens it, and "All agent sessions" at the
  bottom goes to the overview.
- **Workspace card tabs scroll with the mouse wheel.** A long strip of steps moves sideways when the wheel
  turns over it.

## 0.12.1 - 2026-10-07

- **Phone: switching project leaves the old session.** Changing the project in the phone header while a
  session of the previous project was open in the Agents tab kept showing that session. The tab now goes
  back to the session list of the new project.

## 0.12.0 - 2026-10-05

- **Agent accounts share the default memory.** A second Claude Code, Codex or Gemini CLI account no longer
  starts with an empty memory: it reads and writes the default account's (Claude Code per project). Memory an
  account already has is kept. Set `own_memory = true` on an account to keep a memory of its own.

## 0.11.0 - 2026-10-05

- **Plots: several watched values on one chart.** The Live tab's new **Plots** button opens a plot in its
  own panel: pick **New Plot from Watched Values**, or right-click a value and choose **Plot in New Plot** or
  **Add to …**, and the values are drawn together as lines, each in its own colour, with a crosshair and a
  tooltip listing every series, a legend with the latest values, Pause, and a table of the same readings
  (copyable as CSV). Type another variable above the chart to add it; it is watched first if it is not yet.
- **Plots scroll.** Turn the wheel or drag the chart to move back and forth through everything Workbench has
  kept (up to 12,000 readings per value), drag the scroll bar under it, hold Ctrl and turn the wheel to zoom
  (spans from 5 seconds to 30 minutes), and press Follow to go back to the newest readings. Page Up/Down,
  Home, End and `+`/`-` do the same from the keyboard, and the arrow keys carry the crosshair past the edge.
- **Several plots per project, kept by Workbench.** Every plot is a saved configuration (name, series, span,
  scale), so you can keep one per question and dock them side by side, and every browser and device shows the
  same ones: a change in one appears in the others at once. Plots an earlier build kept in the browser move
  over the first time. They are also in the palette (New Plot, Open Plot …).
- **Values of different sizes** share one axis in two ways: **Same axis** (the values' own unit) or **Each as %
  of range** (every series as a percentage of its own range in view), with the real values in the tooltip.
- **Reload keeps the history.** Workbench now keeps the last readings of every watched value, so a reloaded
  page starts with the last minutes, and the Live tab's sparklines show at least the last 30 seconds however
  fast the readings come (they showed the last 120 readings). 64-bit values show every digit in the tooltip,
  table and CSV.
- **Scripts and agents can read them.** `GET …/debug/plots` and `…/plots/<id>/data` (statistics and thinned
  readings of each series), `…/sessions/<id>/live/history` (the raw readings), and a new read-only
  `debug_plots` tool for agents. Only you make and change plots.
- **Live Watch for the rest of the debug servers.** J-Link, pyOCD, st-util, QEMU and an OpenOCD without a Tcl
  port have no way to read memory while the program runs, so the Live tab is now there for them too, with
  values blank until you allow **reading by pausing the program**: Workbench stops the program for a few
  milliseconds, reads and resumes it (every 100 ms or slower). It disturbs the program, so it is off for every
  session until you allow it, and the Live tab shows how long each read stops the program (on a
  NUCLEO-C092RC through OpenOCD about 50 ms, 17% of the time at 250 ms). The stop is not shown as a pause;
  a breakpoint that hits meanwhile, your own Pause or a step of yours is a real motion and is left alone (a read
  never cuts a step short), and a program that is stopped anyway is read for free, so the values follow your steps.
- Smaller fixes found on the way: copying a plot with a very long name no longer freezes the page, the y-axis
  labels no longer clip at seven figures, a project file with one malformed plot no longer loses the project's
  breakpoints and watches with it, and typing a variable while the saved watches are still coming back at the
  start of a session no longer lists it twice.
- Checked on a NUCLEO-C092RC in both themes: through OpenOCD (`ticks` read 1,004 a second against the chip's
  12 MHz clock, the LED's 500 ms toggle drew as a square wave, 50 ms polling over 5 minutes at about 60 frames
  a second) and, through an OpenOCD without a Tcl port, reading by pausing (the session stayed "running",
  nothing was read until allowed, a user pause stayed a real stop, eight steps in a row each ended at their own
  stop, and the cost shown matched the slowdown of the chip's own tick counter). `st-util` 1.8.0 does not know the STM32C092 and wedged the probe: use OpenOCD
  for that chip. Scrolling, keyboard stepping, two browsers editing one plot, a session ending and a new one
  starting, an unwatched series and deleting a plot were checked on a simulated board.

## 0.10.0 - 2026-10-04

- **Live Watch.** The Debug window's new **Live** tab shows variables of the program *while it
  runs*, without stopping it: type a variable (`ticks`, `cfg.limit`, `buf[3]`) or a fixed address
  (`*(uint32_t*)0x50000014`) and press Enter, and its value is read every 250 ms (50 ms to 5 s) and
  drawn as a line of its recent readings. A counter climbs, an LED flag draws a square wave. The
  values come from OpenOCD's Tcl port, which reads memory over SWD while the core runs, so it needs
  OpenOCD (its preset has it; another OpenOCD takes `live_port` in `[debug.servers.<id>]`). Integers,
  floats, booleans, pointers and enums are shown by type, as decimal, hex or binary.
- Only things at a fixed address can be watched: a pointer's target moves, so `*ptr` and `p->x`
  are refused with the reason. A peripheral register that clears when read is refused when the
  project's SVD file says so; the others carry a warning. The expressions are remembered per project
  and come back with the next session, and agents see the latest values in `debug_state`.
- Checked on a NUCLEO-C092RC: a running blinky's `ticks`, `blink_count` and the LED's output register
  read live, in both themes.

## 0.9.2 - 2026-10-04

- **Stop leaves the firmware running.** OpenOCD leaves the core halted when gdb detaches, so Stop
  froze the board (the LED stuck). The OpenOCD preset now makes the target resume on Stop; the new
  `on_stop = "halt"` in `[debug.remote]` keeps the old behaviour. st-util already let the target run.
- **A debug server that goes wrong says so.** The Console explains `unknown chip id` (st-util on a
  chip newer than its tables) when the server prints it, and a probe that stops answering (three USB
  timeouts in half a minute) ends the session at once with the reason, instead of hanging until the
  debugger's own timeout.
- **Windows: debug servers no longer get a port Windows keeps for itself.** Session ports are drawn
  at random, and a draw inside a reserved range (`WinError 10013`) made the server fail to start.
- Finished "Before debugging" tabs of a configuration that was renamed or removed close on the next
  debug run once they are a day old.

## 0.9.1 - 2026-10-04

- **Debug build tabs no longer pile up.** The terminal tab of a configuration's `pre_launch`
  step ("Before debugging: …", and Cargo's "Build …") closed only when you closed it, so every
  configuration left up to three finished tabs in the Agents column. A step that succeeds now
  closes its own tab (its output stays in the terminal history, and the terminal button in the
  Debug window's header opens it), a step that fails keeps its tab so the error can be read, and
  the next run of the same step replaces the old failed tab. A step that left background
  processes running keeps its tab, since closing it would end them.

## 0.9.0 - 2026-10-04

- **Sign an account in from Settings.** **Settings → Agents → Accounts** shows whether each account's
  CLI is signed in ("Signed in · Claude subscription · max", or "Signed out") and has a **Sign in**
  button that runs the CLI's own login in a terminal tab, in that account's folder. Adding a second
  subscription no longer means starting a session to find its login. The status is what the CLI itself
  says (`claude auth status`, `codex login status`); Gemini CLI and Kimi Code cannot say, and are started
  as they are, asking by themselves. Workbench reads no login, token or email, and the sign-in is only
  for a signed-in device, not for an agent.

## 0.8.0 - 2026-10-03

- **Hosted APIs.** An account can use a hosted model API with an API key: DeepSeek, OpenRouter, Z.ai,
  Moonshot, Fireworks, the Anthropic and OpenAI APIs, or any compatible service, for Claude Code,
  Codex and Aider (**Settings → Agents → Accounts → Add…**). The key is named by a `[secrets]` entry,
  read by the server when a session starts, passed in the CLI's environment only and masked in its
  output; the browser sees only the secret's name. Combine them with fallback lists, so a cheap API
  account can take over when a subscription is at its limit.

- **Conversation transfer.** A session can continue on another account with its conversation:
  **Continue on another account…** in its menu, the toast of a session that hit its limit, or
  `failover = "session"`. Between accounts of the same CLI (Claude Code to Claude Code, Codex to Codex,
  also onto a local model) the conversation itself is resumed, by copying the session's file into the
  other account's folder; to another CLI, what was said is written out as text for the new session to
  read. `[agents] transfer = "notes"` carries only a short note. Checked against the real Claude Code
  and Codex: the history reaches the model of the resumed session.
- Local models: an optional **context window** (`local.context`) is passed to Claude Code
  (`CLAUDE_CODE_MAX_CONTEXT_TOKENS`) and Codex (`model_context_window`), and Claude Code's sub-agents
  use the local model too.

- **Local models.** Claude Code, Codex and Aider can run on a model server of your own (Ollama,
  LM Studio, llama.cpp, vLLM…): **Settings → Agents → Accounts → Add local model**, with **Find
  models** to list what the server serves. Nothing of the vendor's login or API key is passed on, and
  a session never falls back to the vendor if the server's setup is incomplete.
- **Usage limits and automatic failover.** Accounts show how full their 5-hour and weekly windows are
  (from Claude Code's status line and Codex's session log) and when a limit ends. Give an account a
  `fallback` list and a new session skips an account that is at its limit: a second subscription, then
  perhaps a local model. `failover = "session"` also continues a running session that hits its limit on
  the next account, as a new session that carries the conversation; otherwise its toast offers
  **Continue on …**.

- **Several accounts of one agent CLI.** Run Claude Code, Codex, Kimi Code, Gemini CLI or
  Aider under more than one login (Aider: more than one `.env` of API keys), such as a work and a personal subscription. **Settings → Agents → Accounts** adds
  an account (a name, a label and a folder of its own); it appears in the new session picker
  beside the CLI, and the composer has an **Account** button for it. Each account's history, Remote Control links and live sessions follow its
  own folder, and Workbench never reads the logins. Accounts were already possible by hand
  in `config.toml`; Remote Control links and the live-session list of an extra Claude account
  were only looked up in the default `~/.claude` before.

## 0.7.1 - 2026-10-03

- **Agents drive the debugger.** New MCP tools: `debug_start` (a configuration by name),
  `debug_attach` (a process), `debug_restart`, and `debug_evaluate` (an expression, or a
  debugger command echoed into your console as `(agent)`); `debug_breakpoints` now accepts
  conditions and log messages. There is no setting to turn on. They are writes (the agent's
  permission prompt applies, Activity marks them) and, unlike the other MCP tools, are not
  confined to the agent's project: `projectId` may name any.
- They run on your computer, outside the sandbox a CLI such as Codex keeps its own commands
  in (a gdb expression can call a shell command, and a start runs your build step and debug
  server). Workbench does not refuse such a session; the CLI's own sandbox and approval
  prompts are what hold it. A pre-launch run that deploys or reaches another host is still
  refused.

## 0.7.0 - 2026-10-02

- **Embedded debugging:** debug firmware on a microcontroller, or in QEMU, from the Debug
  tool window. A `[[debug]]` entry with a `[debug.remote]` table starts a debug server
  (presets for **OpenOCD**, **J-Link GDB Server**, **pyOCD**, **st-util** and **QEMU**; your own
  under `[debug.servers.<id>]` in `config.toml`), waits until its gdb port listens, connects
  gdb to it, resets the target, downloads the program (`load`), resets it again and runs to
  `main` or to your breakpoint. Stop disconnects gdb and ends the server; if the server dies
  or never comes up, the session says why, with the server's own error line.
- **The right GDB for the chip:** Workbench reads the program's ELF header and picks
  `arm-none-eabi-gdb`, a RISC-V or Espressif GDB, or `gdb-multiarch`; new presets
  `gdb-multiarch`, `arm-none-eabi-gdb`, `riscv-gdb` and `xtensa-gdb` appear under Debug
  adapters. A GDB built without Python (which cannot speak DAP) is now reported as unusable
  instead of failing at start.
- **In the window:** configurations of a remote target have a chip icon and a tooltip that
  lists the server's command line and the commands they will run; the Start view lists the
  debug servers it found and how to install the missing ones; the header shows
  `server · target`; the Console shows the server's output in italics and takes `monitor`
  commands; the CPU registers are among the variables. A target can be left halted at the
  reset vector (`stop_at = "reset"`).
- **Agents** can read the server, its output and, with `registers`, the CPU registers of a
  halted core through `debug_state`, and steer a session you started: `debug_control`
  continues, pauses, steps, runs to a line and stops it and answers with the new state;
  `debug_breakpoints` lists and edits plain line and function breakpoints. Both are writes
  (the agent's permission prompt applies, Activity marks them). Agents cannot start,
  attach or restart a session, evaluate expressions, or set conditions and log points:
  a gdb expression can run a shell command, so you do those from the window.
- **Registers by name:** name the chip's CMSIS-SVD file (`svd` under `[debug.remote]`) and
  the Debug window gets a **Peripherals** tab: peripherals, registers and bit fields with
  the vendor's value names, read from the halted target with each register's own access
  size, edited with a double-click (a register, or one field). Registers that change when
  read are left alone until you ask. Checked against ST's STM32F407 SVD and QEMU's SysTick.
- **Target output:** `[[debug.remote.channels]]` shows text the program streams out of band
  (a UART or RTT telnet port, or a decoded SWO/ITM stream) in the Console. QEMU's UART was
  run for real; a real SWO or RTT stream was not.
- **Extended-remote stubs:** `extended = true` connects with `target extended-remote`
  (`gdbserver --multi`, run for real; Black Magic Probe with `attach`, not verified).
- **Dev containers:** a project that runs in its dev container builds there while gdb and
  the debug server stay on this computer; the workspace's paths are mapped for you, and
  `source_map` maps others. Run against a throwaway Debian container.
- `{port2}` to `{port9}` are free ports for a debug server's other listeners.
- A debug server never outlives Workbench: if Workbench crashes or is killed, the system ends
  the server too, so it cannot keep holding the probe.
- Each session gets ports of its own below the system's ephemeral range, so two sessions, or
  an OpenOCD left running, never fight over 3333. The server's *command* comes only from
  `config.toml` or a preset; a repository adds arguments and gdb commands that run when you
  start the configuration.
- See [embedded debugging](docs/embedded-debugging.md). Run end to end with real tools: a
  Cortex-M3 firmware in QEMU through gdb-multiarch, and a host program through `gdbserver`.
  OpenOCD, J-Link, pyOCD, st-util and Black Magic Probe were not run against a chip: their
  commands follow their documentation.

## 0.6.0 - 2026-10-02

- **Updates:** Workbench updates itself. A Workbench installed from a release looks for a
  newer one once a day and shows **Update X.Y.Z** in the status bar; **Settings › Updates**
  has the release notes, **Check now** and **Update and restart**, which downloads the
  release, compares its SHA-256 with the release's checksum, replaces the program and
  restarts into it. The server restarts in place (the same process, a fraction of a second)
  and the page reloads by itself; other open tabs and devices are offered a reload. Before
  it starts it says what the restart stops: agent sessions resume afterwards, shells start
  again under their last screen, runs do not. The replaced version stays as
  `workbench.prev`. The phone's More tab offers the same.
- **`workbench update`** does it from a terminal: `--check` only looks, `--restart` also
  restarts the running Workbench.
- **Restart now** (Settings › Updates) restarts a Workbench whose program was replaced by
  hand, with `install.sh` or `workbench update`.
- Nothing is installed without your click, and agents cannot update or restart Workbench.
  Looking is one request a day to GitHub without a token; `[update] check = false` turns
  it off. A build from source has no release to look for until `[update] repo =
  "owner/name"` names a repository. On Windows, and where Workbench cannot write to its
  own folder, it tells you about the new version and leaves installing to the archive's
  installer.

## 0.5.3 - 2026-10-02

- **Agents column:** a session opened from an attention toast, the history, a Workspace card
  or a notification is shown in the column of its own project: Workbench switches to that
  project instead of adding the tab to the project on screen, so the columns no longer mix
  sessions of different projects. Tabs an earlier version added that way are moved on load.

## 0.5.2 - 2026-10-01

- **Terminal tool window:** the **Terminal** button is back at the foot of the left stripe,
  first of the bottom group. It opens shells under the editor, as tabs, beside whatever the
  agents column shows; **+** starts one (in the dev container or on the host when one runs).
  Those shells are the tool window's own: the agents column leaves them out, and opening one
  of them from anywhere shows it there. Shells started from the column stay in the column.

## 0.5.1 - 2026-10-01

- **Docs:** the tour card's screenshots (CI tests, a Workspace card, Services, Database), the
  help pages, the welcome card and the README describe the agents column: sessions start
  from the first tab of the column, the project switcher sits over it, Alt+F12 collapses the
  workspace window.

## 0.5.0 - 2026-09-30

- **Layout:** agents and terminals are a column on the left of the window, no longer tabs
  of the centre. Its first tab has the prompt for a new session, **New shell** and the
  project's sessions; every agent session, shell and run output is a tab next to it, and
  **+** starts a session or a shell. Closing a tab stops what runs in it (a session stays in
  the history). 
- **Two windows:** the agents window (with the project switcher on top) and, beside it, the workspace window with everything else; the branch, run configurations, environments, CI and Commands moved into the workspace window's top bar. The button at the top of the agents window (Alt+F12) collapses the workspace window, so the agents have the whole width; in an app window the browser window shrinks to the agents window at the same position, and grows back when you expand it. A session's header wraps onto two rows in the narrow agents window instead of cutting its title. The session tabs follow the Agents button left to right and take as many rows as they need, under that button too, instead of scrolling out of sight in one row; and the "2 working" / "1 needs you" pill sits at the right end of the agents window's top bar.
- **Tool windows:** the icon stripe sits right of that column. **Workspace** is its first
  button, above Files, and a project opens on its Workspace cards. **Settings** moved to the
  foot of the stripe. The Agents and Terminal tool windows are gone: the column replaces
  them. Layouts saved by earlier versions are carried over (their agent and terminal tabs
  leave the centre).

## 0.4.1 - 2026-09-30

- **Add project** has a **Browse…** button: a folder picker on the Workbench computer (subfolders only, hidden ones on request) that fills the path field.

## 0.4.0 - 2026-09-30

- **Sandbox:** the Workspace has a Sandbox scope (`wb-sandbox`) for throwaway documents,
  reports and agent experiments. It has its own tab in the Workspace tool window, the phone
  list, the Workspace home and the New card dialog, a guide card with a playground report that
  shows what a sandboxed report can reach, and **Reset**, which moves every card to the trash
  and brings the guide back. Its cards stay out of the "All" view, and agents may write to it.
- **Tabs follow the project:** each project keeps its own tabs and splits; switching projects
  switches them.
- **Add project** offers to create the directory when it does not exist, and switches to the
  new project.
- **Models:** `fable` is offered when starting a Claude Code session and in Settings › Agents.
- **Settings** moved to the top left of the window.

## 0.3.1 - 2026-09-30

- **Agent sessions (security fix):** removing an agent session from history while a restart
  or a restore was waiting to start it could leave a working agent token behind, valid until
  Workbench restarted. For Claude Code, the session's files also came back with that token
  in them: `claude-settings.json` and `mcp.json` in the data directory, or inside the
  container for a session in a dev container. A restart or restore that was waiting now
  finds the session gone and starts nothing, so it issues no token and writes no files. An
  agent session that fails to start no longer keeps the token it was given either. An agent
  token gives what its session has: Workbench's hooks and its MCP tools.
- **Runs:** a run whose terminal is closed or whose process is ended from outside now ends
  "exited" with a "terminated" warning (after its test counts, if any), and its toast says
  it was terminated, not "failed: exited with code 1". That covers Kill, Close and Restart
  in Workbench and, on Linux, a hang-up, terminate, kill or interrupt signal that ends its
  process, whoever sends it (Ctrl+C in its terminal, or the kernel's out-of-memory killer).
  A crash (such as a segmentation fault or an abort), a non-zero exit code and tests that
  failed before the end still fail, and Stop still stops. A run cut short does not count as
  finished for runs that depend on it, a debug session's pre-launch run says it was
  terminated, and agents see `terminated` in `run_list`. On Windows only Workbench's own
  closes are known: a process ended from Task Manager exits with code 1 and still reads as
  failed.
- **Deploys:** a deploy no longer reports "the repository has no commits" (or "unknown
  commit") when git itself failed. It gives git's own message, such as "not a git
  repository", or says that the folder is gone or that git is missing or timed out. "No
  commits" and "unknown commit" now mean that git ran and found no such commit.
- **Line endings:** Workbench follows git's line-ending conversions on Linux too, as it
  already did on Windows. They apply to a file git reads with LF although it has CRLF on
  disk: checked out with CRLF over LF in the repository (`eol=crlf` in `.gitattributes`,
  `core.autocrlf=true`), or saved with CRLF where git converts it (a `text` attribute,
  `text=auto`, `core.autocrlf=input`). Its diff shows the working tree as git reads it, so
  once every change is staged it says "Nothing unstaged in this file." instead of showing
  two identical sides. A conflict in it is shown with LF, like both sides, and the text you
  resolve it with is written back with CRLF (a file with mixed line endings gets the text as
  sent). Local History keeps the "Last commit (HEAD)" of a CRLF checkout with CRLF, so
  comparing it with the first change shows the edited lines, not every line, and a file
  that only went through the checkout no longer gets that entry. Files git does not convert
  keep their bytes: LF files, `-text` files, and CRLF committed as is under `text=auto` or
  `core.autocrlf`. A `text` attribute, or `eol` without `text=auto`, has git convert CRLF
  committed as is too and show every line changed until the file is renormalized;
  Workbench's working-tree side now has LF to match.
- **File names with `\` (Linux):** a `\` in a file or folder name is part of the name, as
  Linux has it. Workbench used to read it as `/`, so `a\b.txt` became the file `b.txt` in
  the folder `a`: the files in a folder named `d\x` opened as `d/x/…`, search and quick
  open results (and Replace in Files from them) led to that other file, and Local History
  and an agent's `workbench_open_file` kept or opened its path. The file tree, opening,
  saving, renaming, search, quick open, the watcher, Local History and detected run folders
  now keep the name. On Windows `\` still separates.
- **Git messages:** error boxes keep the line breaks of multi-line messages, such as git's.
  A fetch, update, push or remote-branch deletion that fails because ssh would have had to
  ask something now says what to do: for an unknown host key, connect once with ssh in a
  terminal, check the fingerprint and accept the key; for `Permission denied (publickey)`,
  load a key that has a passphrase into ssh-agent (on Windows, start the OpenSSH
  Authentication Agent service first), or else add your public key to your account on the
  server. A changed host key is flagged with a warning to check its fingerprint before
  replacing it, and a revoked one with a warning not to trust it again. When git is missing
  or times out, the GitLab and GitHub pollers still watch only the default branch, and now
  log why once per project.
- **Machine overlays:** an overlay that does not parse or cannot be read is still left out
  whole, secrets included, but a secret missing for that reason now comes with the
  overlay's error (for a parse error, its line and column) instead of the advice to add it
  under `[secrets]` in that same overlay. The usual cause is a Windows path in double quotes
  (`"C:\Users\…"`, where `\` starts an escape);
  [Make Workbench yours](docs/customization.md#keep-secrets-where-they-are) shows the
  spellings that work.
- **Server log:** colour escapes only on a terminal. The systemd journal, output redirected
  to a file or a pipe, and the Windows `service.log` get plain text. `NO_COLOR` still turns
  colour off on a terminal.
- **MCP:** the `workbench_notify` description says what it does: a toast in Workbench, plus
  a desktop notification, your notify command and a push to your devices where you set them
  up (Windows has no desktop notifications yet).
- **API:** a terminal's `exit` (also in `terminal.exited`) and a run (`GET …/runs`,
  `run.state`) have `terminated: true` when they were cut short. `POST …/deploy/check` and
  `…/deploy` answer a git failure with `422 git_error`, `504 timeout`, `412 not_configured`
  (no git) or, on Windows, `403 unsafe_repository`, no longer `400 bad_request`. On
  Windows, a delete the Recycle Bin would not take answers `422 not_recyclable`, not 500.
- **Windows (experimental):** still experimental. Its test suite passes on GitHub's
  `windows-latest` (Windows Server 2025); it has not been tried on a Windows 10 or 11
  desktop yet ([status](docs/windows-port.md)).
- **Run configurations (security fix, Windows):** detection no longer offers a run whose
  command quotes a name from a repository file that has a space and ends in `\`. Windows
  PowerShell 5.1, which runs commands wherever PowerShell 7 is not installed, lets that `\`
  escape the closing quote, so the next quoted name is split into arguments of its own:
  `cargo run -p 'x \' --bin 'y --z'` would hand cargo `--z`.
- **On Windows:** a repository git refuses because another user owns the folder
  (`safe.directory`) no longer just loses its branch. The project shows a warning that names
  the folder and the command that trusts it, the status bar reads "Untrusted repository",
  and the git tool windows show git's message with a button that copies the command.
  Deploys report the refusal too, and the pollers log it once per project.
- **On Windows:** programs installed while Workbench runs (Node.js, Python, rustup, an agent
  CLI, a language server) are found without a restart. New terminals, runs and agent
  sessions get the `PATH` a new sign-in gets, followed by the folders of Workbench's own
  `PATH` that it lacks (such as a virtual environment Workbench was started from).
  Workbench's own lookups (language servers, debug adapters, agent CLIs) try it too, and
  what Workbench starts by itself gets its new folders after Workbench's own, so a newly
  installed gopls finds a newly installed Go. A newly installed Git for Windows is still
  found only after a restart. `workbench service install --enable` and
  `workbench service open` start Workbench in your sign-in environment, not in that of the
  shell they run in.
- **On Windows:** a file outside the project at a drive path (a language server's
  definition in a library, a debug stop in `C:\…`) opens instead of being refused. Copy
  Path and drag and drop join paths with `\`, and tab titles, breadcrumbs and stack frames
  name a file by what follows its last `\`.
- **On Windows:** paths other programs write compare as Windows compares them (any case,
  `\` or `/`): Claude Code's project entries in `~/.claude.json`, Gemini's chat folders,
  the session folder that shortens the paths in a permission prompt, and a file an agent's
  hook names in another case, which no longer starts a second Local History. The
  debugger's attach picker knows a process by its image name (`node.exe`, `javaw.exe`) and
  reads `C:\Program Files\…` command lines, and a launch configuration's `.\cmd\api` in a
  Go module debugs as Go.
- **On Windows:** a terminal whose own process has exited while something it started runs
  on (a background job, a program it opened) no longer keeps a thread waiting for those to
  end. One thread hears from Windows, for every terminal, when the last of them has ended,
  and checks every second in case that report does not come.
- **On Windows:** deleting a file or folder whose own path is 260 characters or longer no
  longer fails with a server error. It goes to the Recycle Bin where Windows takes it;
  where it does not, nothing is deleted and the message says why and how to delete it
  instead (shorten the path, or delete it for good from a terminal). A drive without a
  Recycle Bin, a bin turned off, or a file larger than the bin gets the same kind of
  answer.
- **On Windows:** `install.ps1 -Uninstall` (with the `-Prefix` you installed with) removes
  Workbench: the services started from its folder, the files the install put there, the
  `PATH` entry it added, and the folder once nothing else is in it. It keeps your
  configuration and data, and changes nothing while Workbench runs from that folder, or
  when it cannot tell. The executables carry an application manifest (Windows 10 and 11,
  message boxes in the current style, long paths where Windows allows them).
- **Dev containers on Windows:** the install notes, the Help's Agents page and the "not
  available on Windows" message now say how to get them: run the Linux build inside a WSL 2
  distribution with Docker Engine installed in that distribution, and keep its projects in
  the distribution's own folders. A browser on Windows then opens only ports the container
  publishes (`forwardPorts`, `appPort`). With Docker Desktop's WSL integration instead,
  Workbench may be unable to listen on the container network's gateway, and then refuses
  Claude Code sessions inside the container. Neither setup has been tested yet.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.3.1-x86_64-unknown-linux-gnu.tar.gz` and run
`./workbench-0.3.1-x86_64-unknown-linux-gnu/install.sh`.

Windows 10 (1809 or newer) or 11 on x86_64, experimental: in PowerShell, run
`Unblock-File` on `workbench-0.3.1-x86_64-pc-windows-msvc.zip`. It removes the Mark of the
Web that Windows puts on downloads, so the unpacked files do not carry it. Unpack the zip
with `Expand-Archive .\workbench-0.3.1-x86_64-pc-windows-msvc.zip -DestinationPath .`, then
run its installer:
`powershell -ExecutionPolicy Bypass -File .\workbench-0.3.1-x86_64-pc-windows-msvc\install.ps1`
(`-ExecutionPolicy Bypass` allows the unsigned script for this one run). It installs into
`%LOCALAPPDATA%\Programs\Workbench` without administrator rights and adds that folder to
your PATH. Then run `workbench serve --open` in a new terminal. The binaries are not
code-signed: SmartScreen or an antivirus may warn, and Windows 11's Smart App Control, when
on, blocks them. To remove Workbench, stop it (`workbench service stop`, or Ctrl+C), then
run the same command with `-Uninstall` from a terminal outside Workbench.

Each archive has a `.sha256` to check it against (`sha256sum -c`, or `Get-FileHash` on
Windows).

**Update:** from 0.3.0 or earlier, install the new archive (or pull and rebuild), then
restart Workbench (`systemctl --user restart workbench.service` or the running
`workbench serve`). The restart also ends any agent token an earlier version left behind.
Your configuration and data stay as they are. On Windows, 0.3.1's `install.ps1` installs
over 0.3.0, also while Workbench runs. Then restart Workbench:
- If you started it with `workbench serve`, stop it (Ctrl+C) and start it again in a new
  terminal.
- If it runs as the service (from the Start Menu or at sign-in), run
  `workbench service stop` and open Workbench from the Start Menu.

## 0.3.0 - 2026-09-29

- **Windows (experimental):** the first release with a Windows build, for Windows 10 (1809
  or newer) and 11 on x86_64. Its whole test suite passes on GitHub's `windows-latest`
  (Windows Server 2025), a required CI job, and the release job installs the build with
  `install.ps1`, starts the server and installs again over it before publishing the
  archive. It has not been tried on a Windows 10 or 11 desktop yet
  ([status](docs/windows-port.md)). The archive holds `workbench.exe` (no Visual C++
  runtime needed), `workbenchw.exe`, `install.ps1`, and `conpty.dll` and `OpenConsole.exe`
  from Microsoft's ConPTY package (MIT, see the third-party notices).
- **On Windows:** terminals, agent sessions included, run in ConPTY. Shells are
  PowerShell 7 (`pwsh`), else Windows PowerShell. Agent CLIs start directly, and
  npm-installed ones start as `node` and their script, never through cmd.exe. Run
  configurations and detected commands also run in PowerShell, so a command written for
  bash needs PowerShell's syntax (Windows PowerShell 5.1 has no `&&`: install
  PowerShell 7). Language servers and debug adapters are found in their Windows forms
  (npm-installed ones run with Node). Rust built with MSVC debugs with lldb-dap or
  CodeLLDB, which you name in `[debug.default_adapter]`, since gdb reads only MinGW builds.
  `workbench service install` adds a Start Menu shortcut, and with `--enable` a sign-in
  entry, without administrator rights. The configuration is in `%APPDATA%\workbench` and
  the state in `%LOCALAPPDATA%\workbench`, both readable only by you and SYSTEM. A
  `keyring` secret reference reads Windows Credential Manager. Nothing Workbench reads in a
  project by itself follows a link to a network path or a device (`\\host\share\x`), which
  would make Windows sign in to that computer. Programs it starts (git, language servers,
  agents, your terminals) are not covered. What else works differently there, such as a
  terminal ending a browser or editor it started, is in
  [Install on Windows](docs/getting-started.md#install-on-windows-experimental).
- **Left out on Windows:** dev containers, the server's desktop notifications (turn on
  browser notifications instead), gdb attaching to a running process (native programs
  attach with lldb-dap or CodeLLDB, Python with debugpy), rust-gdb's pretty printers, and
  projects on a network share or inside WSL. The Services window (Docker Desktop) is
  marked experimental. Workbench hides these or says "Not available on Windows" and why
  (the API answers `unsupported_platform`, and `GET /api/health` lists them).
- **Git on Windows:** version control needs Git for Windows. A CRLF checkout
  (`core.autocrlf`, Git for Windows' default, or `eol=crlf` attributes) diffs as git reads
  it, with LF. A conflict resolved with edited text is written back with CRLF, and Local
  History keeps the last commit with the checkout's line ends. A repository that git
  refuses because of its owner (`safe.directory`) reports git's own message
  (`unsafe_repository`), which names the command that trusts it. Git Credential Manager
  never opens a sign-in window for Workbench's remote operations. It answers with what it
  has stored, and an https host it has nothing for fails at once.
- **Terminals:** `[terminals] shell` in `config.toml` sets the program and arguments of new
  shells (default: `$SHELL -l`, as before; PowerShell on Windows). Kill, Restart and Close
  now wait (up to 5 seconds) until the process's exit is recorded and saved. Before, they
  waited only until the process ended, or not at all for a process that had just ended by
  itself. Once they return, the terminal reads as exited. A terminal removed from history
  stays removed: a save still under way no longer writes its files back, so the terminal
  does not return at the next start.
- **Git credentials (security fix):** Workbench no longer answers a credential prompt
  whose user name contains `/`. Git before its January 2025 security releases prints that
  name unescaped, so a crafted remote or submodule URL could get the GitLab token sent to
  another host. Workbench's fetch, update, push and remote-branch deletion also no longer
  ask git's credential helpers for the GitLab host Workbench has a token for (the
  project's own `[repo.gitlab]` token, else `[gitlab]`), nor hand them that token to
  store. The token no longer ends up in Git Credential Manager, `~/.git-credentials`, a
  credential cache or a keychain. While Workbench has a token for that host, a stored
  credential no longer answers in its place, so a project's own token or a rotated one
  takes effect at once. Other hosts keep your helpers. If a credential helper of yours
  knew the GitLab host, Workbench's remote operations now use Workbench's token there, and
  that token needs Git-over-HTTPS access (write access to push). A token that earlier
  versions left with a credential helper stays there: remove it (for `store`, the GitLab
  host's line in `~/.git-credentials`).
- **Run configurations (security fix):** Unity detection takes the editor version from
  `ProjectVersion.txt` only when it consists of version characters (letters, digits, `.`,
  `_`, `-`). Otherwise a crafted `ProjectVersion.txt` could put shell syntax into the
  detected Unity runs.
- **Setup messages:** several messages now name the project's machine overlay,
  `config.toml` and the config folder this Workbench actually reads (`WORKBENCH_CONFIG_DIR`,
  `XDG_CONFIG_HOME`) instead of always `~/.config/workbench`. They are the messages about a
  missing toolchain, an unknown placeholder or an undefined ssh host, the Confluence setup
  hint and its "not available" message, and the TLS certificate and key placeholders in
  Settings › Remote. A default Linux install shows the same text as before.
- **API:** `GET /api/health` also reports `os`, what that OS leaves out (`unsupported`) and
  what it has only as experimental (`experimental`). Both are empty on Linux. An
  `unsupported_platform` error (HTTP 501) names its `feature`. `GET /api/atlassian/status`
  also returns `configFile`, the `config.toml` this server reads.
- **Build from source:** the build also makes `workbenchw`, the Windows launcher of
  `workbench service`. On Linux it is a stub that only prints a message, so install
  `workbench` alone, as before. The Linux archive still holds `workbench` alone.
- **Docs:** Help and the guides cover Windows (install, folders, the service, agents, what
  is left out). customization.md and ARCHITECTURE.md now give the `dotenv` secret
  reference in the form Workbench reads: `dotenv = { path = "…", key = "…" }`.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.3.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`.

Windows 10 (1809 or newer) or 11 on x86_64, experimental: in PowerShell, run
`Unblock-File` on `workbench-0.3.0-x86_64-pc-windows-msvc.zip`. It removes the Mark of the
Web that Windows puts on downloads, so the unpacked files do not carry it. Unpack the zip
with `Expand-Archive .\workbench-0.3.0-x86_64-pc-windows-msvc.zip -DestinationPath .`, then
run its installer:
`powershell -ExecutionPolicy Bypass -File .\workbench-0.3.0-x86_64-pc-windows-msvc\install.ps1`
(`-ExecutionPolicy Bypass` allows the unsigned script for this one run). It installs into
`%LOCALAPPDATA%\Programs\Workbench` without administrator rights and adds that folder to
your PATH. Then run `workbench serve --open` in a new terminal. The binaries are not
code-signed: SmartScreen or an antivirus may warn, and Windows 11's Smart App Control, when
on, blocks them.

Each archive has a `.sha256` to check it against (`sha256sum -c`, or `Get-FileHash` on
Windows).

**Update:** install the new release (or pull and rebuild), then restart Workbench
(`systemctl --user restart workbench.service` or the running `workbench serve`). Your
configuration and `~/.local/share/workbench` stay as they are. On Windows, a later
archive's `install.ps1` installs over this one, also while Workbench runs. Then restart
Workbench:
- If you started it with `workbench serve`, stop it (Ctrl+C) and start it again in a new
  terminal.
- If it runs as the service (from the Start Menu or at sign-in), run
  `workbench service stop` and open Workbench from the Start Menu.

## 0.2.0 - 2026-09-29

- **Help:** the user documentation in the app, bundled so it works offline and on a phone:
  getting started, projects, agents, version control and CI, remote access and the phone
  (including Tailscale), running as a service, and `config.toml`. Open it with F1, the
  palette, the status bar's Help, or More → Help on a phone; search covers every page.
- **Docs:** a plan for porting the server to Windows ([windows-port.md](docs/windows-port.md)).

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.2.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`, then restart
Workbench (`systemctl --user restart workbench.service` or the running `workbench serve`).

## 0.1.0 - 2026-09-29

First public release.

- **Agents:** Claude Code, Codex, Kimi Code, Gemini CLI, Aider and custom CLIs in real
  terminals; permission requests answered from cards, toasts and the phone; Review Changes
  with per-file and whole-session revert; Claude Code Remote Control; MCP tools for the
  UI, CI, Confluence and Jira, Workspace cards, runs, code intelligence, the debugger and
  Local History.
- **Editor:** Monaco on CLion's keymap, language servers with semantic highlighting,
  hierarchies and structure (presets from rust-analyzer and clangd to Verible for
  Verilog and SystemVerilog and vhdl_ls for VHDL), a DAP debugger, Local History,
  bookmarks, compare, scratch files and an HTTP client for `.http` files.
- **Version control:** line staging and partial commits, changelists, the shelf,
  interactive rebase, bisect and a log graph.
- **Forges:** GitLab merge requests, pipelines, jobs and test reports; GitHub pull
  requests, Actions runs and artifacts; issues on both.
- **Atlassian:** Confluence reading and authoring with inline comments, attachments and
  labels; Jira issues and boards.
- **Running things:** detected run configurations, environments with health checks,
  previews and gated deploys, dev containers, a Services window for Docker and a Database
  window for PostgreSQL.
- **Workspace:** deliverable cards with sandboxed reports, galleries, PDFs and 3D
  comparisons, compatible with Mr. Mak Workspace's registry; four example cards in Home
  on the first start.
- **Phone:** an installable app with its own tabs and push notifications.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.1.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`, or build from
source. Windows is not supported yet.

**Update:** install the new release (or pull and rebuild), then restart Workbench
(`systemctl --user restart workbench.service` or the running `workbench serve`). Your
configuration and `~/.local/share/workbench` stay as they are.
