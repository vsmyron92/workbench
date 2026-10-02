# Embedded debugging

Workbench debugs firmware on a microcontroller, or in a simulator, from the Debug tool
window: breakpoints in the editor, the call stack, variables, the CPU registers, stepping,
and the debug console where gdb's `monitor` commands work. It goes through two programs
you install:

- **a GDB 14 or newer built with Python.** gdb's own Debug Adapter Protocol server is what
  Workbench talks to.
- **a debug server**, the program that sits between gdb and the chip: OpenOCD, SEGGER's
  J-Link GDB Server, pyOCD, `st-util`, or QEMU standing in for a board.

Starting the configuration runs, in order: your build step, the debug server, gdb; gdb
connects to the server (`target remote`), the target is reset, the program is downloaded
to it (`load`) and reset again, and it runs to `main` or to the breakpoint you set. Stop
disconnects gdb and ends the server.

> **What has been run.** QEMU (a Cortex-M3 board, with its UART and SysTick), `gdbserver`
> and `gdbserver --multi` were run end to end with real tools, and OpenOCD was started for
> real (without a probe). The OpenOCD, J-Link, pyOCD, `st-util` and Black Magic Probe
> setups follow those tools' documentation; they have not been run against a chip here.
> [Limits](#limits) lists what is verified and what is not. If a preset's default commands
> do not fit your board, override them (below): they are plain gdb commands.

## What to install

| You need | Install |
| --- | --- |
| A GDB for your chip | `gdb-multiarch` (Debian and Ubuntu: `sudo apt install gdb-multiarch`) debugs Arm, RISC-V, Xtensa and more. Or your toolchain's own: the Arm GNU Toolchain 13.3 or newer (`arm-none-eabi-gdb`), an xPack or SDK RISC-V GDB, Espressif's `xtensa-esp32-elf-gdb` / `xtensa-esp-elf-gdb`. |
| A debug server | OpenOCD (`sudo apt install openocd`), SEGGER's J-Link software pack, pyOCD (`pipx install pyocd`), stlink-tools (`st-util`), QEMU (`qemu-system-arm`). |
| USB access to the probe | On Linux a udev rule for your user (OpenOCD ships `60-openocd.rules`; most probe packages install one). Without it the server reports that it cannot open the probe, and that line is in the console. |

The Debug tool window's **Start** view lists the GDBs under *Debug adapters* and the
servers under *Debug servers (embedded targets)*, what it found and what to do about what
it did not. A GDB older than 14, or built without Python (some vendor toolchains), is
reported as unusable: gdb-multiarch is the usual way out.

## A first configuration

Add this to the repository's `.workbench.toml` (or to your machine overlay,
`~/.config/workbench/projects/<id>.toml`):

```toml
[[debug]]
name = "Blinky (OpenOCD)"
program = "build/blinky.elf"
pre_launch = "cmake --build build"
stop_on_entry = true                # stop at main

[debug.remote]
server = "openocd"
server_args = ["-f", "interface/stlink.cfg", "-f", "target/stm32f4x.cfg"]
```

Press **Shift+F9**. The configuration appears in the Start view with a chip icon; its
tooltip shows exactly what it will run: the server's command line, where gdb connects,
the commands and what happens next.

Without `stop_on_entry` the program runs until a breakpoint. Breakpoints can be set before
or after the session starts; they are placed as soon as gdb has the program's symbols.

## The `[debug.remote]` table

| Field | Meaning |
| --- | --- |
| `server` | The debug server to start: `openocd`, `jlink`, `pyocd`, `st-util`, `qemu-arm`, or one you define in `config.toml` (below). Leave it out when the stub already runs and `connect` says where. |
| `server_args` | Arguments after the server's own: the probe, the chip, the board. `{program}`, `{root}`, `${workspaceFolder}` and your toolchain placeholders expand. |
| `connect` | gdb's `target remote` argument: `host:port`, or a serial device. Default: the server's port on this computer. A loopback port (`localhost:3333`) is also the port the server is told to listen on. |
| `port` | The port the server's gdb stub listens on. Default: a free one, picked for every session, so two sessions (or an OpenOCD you left running) never fight over 3333. |
| `init` | gdb commands run once connected, before the download. One command or a list. |
| `reset` | gdb commands that reset and halt the target; run before the download and again after it. |
| `download` | Whether to download the program to the target (`load`). |
| `source_map` | Where the source was when the program was built, as `[from, to]` pairs: `source_map = [["/workspaces/app", "{root}"]]`. gdb's `set substitute-path`, set before the program is read, so breakpoints and the editor find files whose paths the ELF records somewhere else (a CI build, a SDK). With a dev container (below) its workspace pair is added for you. |
| `svd` | A CMSIS-SVD file, the chip vendor's register map (project-relative, absolute or `~/`). It adds the **Peripherals** tab ([below](#peripherals-registers-by-name)). |
| `channels` | Text the program streams out of band (RTT, SWO, a UART on a socket): `[[debug.remote.channels]]`, [below](#output-channels-uart-rtt-swo). |
| `extended` | Connect with `target extended-remote`: `gdbserver --multi` and Black Magic Probe, [below](#extended-remote-black-magic-probe-gdbserver---multi). With `attach` (a process id or a probe's target number) and `exec_file`. |
| `stop_at` | Where the program stops first: `main` (what `stop_on_entry` means), `reset` (halted at the reset vector, for debugging startup code) or any gdb location (`app_main`, `src/main.c:42`). Naming a place stops there without `stop_on_entry`. |

`init` and `reset` take their defaults from the server; an empty list (`reset = []`)
means "none". The other fields of `[[debug]]` work as usual: `program` (the ELF),
`pre_launch` (your build: a run configuration's name or a command), `cwd` (where the server
runs), `env`, `adapter`.

### What the presets run

| Server | Command | Default `reset` | `download` |
| --- | --- | --- | --- |
| `openocd` | `openocd -c "gdb_port {port}" -c "telnet_port {port2}" -c "tcl_port {port3}"` + your `server_args` | `monitor reset halt` | yes |
| `jlink` | `JLinkGDBServerCLExe -nogui -port {port} -swoport {port2} -telnetport {port3}` (`JLinkGDBServerCL` on Windows; `/opt/SEGGER/JLink` is searched when it is not on `PATH`) | `monitor reset`, `monitor halt` | yes |
| `pyocd` | `pyocd gdbserver --port {port} --telnet-port {port2}` | `monitor reset halt` | yes |
| `st-util` | `st-util -p {port}` | none | yes |
| `qemu-arm` | `qemu-system-arm -S -gdb tcp:127.0.0.1:{port} -display none -monitor none -serial stdio` | none | no (QEMU loads the image) |

`{port2}` to `{port9}` are free ports for the server's other listeners (telnet, SWO, Tcl,
RTT): every session gets its own, and a placeholder you use is a port Workbench reserved
for you.
The presets move the listeners they know about off their well-known defaults (OpenOCD's
3333, 4444 and 6666; J-Link's 2331 to 2333), so sessions can run side by side. J-Link's RTT
port (19021) is not moved: two J-Link sessions at once would need `-RTTTelnetPort` in
`server_args`.

## Examples

**ST-LINK on an STM32 (OpenOCD).** The first example above. Board files work too:
`server_args = ["-f", "board/st_nucleo_f4.cfg"]`.

**J-Link.**

```toml
[debug.remote]
server = "jlink"
server_args = ["-device", "STM32F407VG", "-if", "SWD", "-speed", "4000"]
```

**pyOCD.**

```toml
[debug.remote]
server = "pyocd"
server_args = ["--target", "stm32f407vg"]
```

**QEMU, no hardware at all.** A Cortex-M3 board, with the serial port's output in the
debug console:

```toml
[[debug]]
name = "Firmware (QEMU)"
program = "build/firmware.elf"
pre_launch = "cmake --build build"
stop_on_entry = true

[debug.remote]
server = "qemu-arm"
server_args = ["-M", "lm3s6965evb", "-kernel", "{program}"]
```

**A stub that is already running** (a board's `gdbserver`, a probe you started by hand):

```toml
[[debug]]
name = "Board (gdbserver)"
program = "build/app"

[debug.remote]
connect = "board.local:2345"
download = false
```

**ESP32 and RISC-V parts.** Name the GDB and the board files; the rest is the same. (ESP32
needs Espressif's OpenOCD build: point `[debug.servers.openocd] command` at it.)

```toml
[[debug]]
name = "ESP32"
adapter = "xtensa-gdb"
program = "build/app.elf"

[debug.remote]
server = "openocd"
server_args = ["-f", "board/esp32-wrover-kit-3.3v.cfg"]
```

## Extended-remote: Black Magic Probe, `gdbserver --multi`

Some stubs are connected with `target extended-remote` instead of `target remote`: the
stub outlives the program, and either runs it for you or attaches to something it found.
Set `extended = true`.

**A stub that runs the program** (`gdbserver --multi`). Workbench connects, tells the stub
which file to run (`exec_file`, default `program`) and starts it, stopping at `main`:

```toml
# config.toml
[debug.servers.gdbmulti]
label = "gdbserver --multi"
command = "gdbserver"
args = ["--multi", "127.0.0.1:{port}"]
download = false
```

```toml
# .workbench.toml
[[debug]]
name = "On the board"
program = "build/app"
stop_on_entry = true
[debug.remote]
server = "gdbmulti"
extended = true
```

**A probe that scans for targets** (Black Magic Probe): `init` scans, `attach` is the
number the scan lists. The probe is a serial device, so there is no server to start:

```toml
[debug.remote]
connect = "/dev/ttyACM0"
extended = true
init = ["monitor swdp_scan"]
attach = 1
```

`init` runs after the connection and before the attach. A stub that runs the program
neither resets the target nor downloads anything (nothing is on a chip to download to);
with `attach`, `reset` and `download` apply as for any other target.

## Output channels: UART, RTT, SWO

A firmware's `printf` rarely goes through gdb. It goes out of a UART, over SEGGER RTT, or
through the SWO pin. Servers and simulators expose these as TCP ports; a channel tells
Workbench to read one and show the bytes in the Console, in the program's colour:

```toml
[debug.remote]
server = "jlink"
server_args = ["-device", "STM32F407VG", "-if", "SWD", "-RTTTelnetPort", "{port4}"]
channels = [
  { name = "RTT", port = "{port4}" },                      # J-Link's RTT telnet port
  { name = "SWO", port = "{port2}", format = "itm" },      # the preset's own -swoport {port2}
]
```

- `port` is a port number, or a placeholder (`"{port2}"` to `"{port9}"`, never the gdb
  stub's `{port}`) for one of the free ports Workbench picked for the session. The server's
  arguments must use that placeholder too (that is how the server learns the port); the
  Start view reports one that nothing uses. At most eight channels.
- `format = "itm"` decodes a SWO stream: the bytes of one stimulus port (`itm_port`,
  default 0) are the text. Plain `text` is the default.
- A channel connects when the server is up and keeps trying, since RTT's port opens when
  the program starts and a reset closes it. A channel that cannot be reached never holds
  the session up or fails it.
- With several channels each line starts with the channel's name, `[RTT] `. The Console
  says when a channel connects and when the server closes it.
- QEMU's first serial port is already in the Console through the `qemu-arm` preset
  (`-serial stdio`, shown as the server's output). To keep it apart, define a server that
  serves it on a port (this is the setup that was run against QEMU's Cortex-M3):

  ```toml
  # config.toml
  [debug.servers.qemu-uart]
  label = "QEMU (UART on a port)"
  command = "qemu-system-arm"
  args = ["-S", "-gdb", "tcp:127.0.0.1:{port}", "-display", "none", "-monitor", "none",
          "-serial", "tcp:127.0.0.1:{port2},server=on,wait=off"]
  download = false
  ```

  and `channels = [{ name = "UART", port = "{port2}" }]` in the configuration.
- The channels' text is part of what an agent reads through `debug_state` and of
  *Ask agent about this stop*.

## Peripherals: registers by name

A chip vendor's **CMSIS-SVD** file names every peripheral, register and bit field of the
chip. Name it, and the Debug window gets a **Peripherals** tab:

```toml
[debug.remote]
server = "openocd"
server_args = ["-f", "board/st_nucleo_f4.cfg"]
svd = "svd/STM32F407.svd"
```

- The tab lists the chip's peripherals (filter by name or description); open one to read
  its registers from the halted target, each with its address, value and, opened, its
  fields with the vendor's value names (`CLKSOURCE = Processor (1)`). Values that changed
  since the last stop are highlighted.
- Double-click a register to write it (a number, `0x…`), or a field to change just that
  field: a field with named values opens a menu of them.
- Registers are read with the access size the SVD gives them (a 16-bit register is read
  as 16 bits, never as part of its neighbour), once per stop, only while the program is
  suspended.
- **A read can change the chip**, such as a status flag that clears when read. The SVD
  says so (`readAction`); those registers are left alone until you press the eye button on
  their row, and a field of one cannot be changed alone (that would read it): write the
  whole register. Write-only registers are never read.
- The routes behind it (`GET …/svd`, `GET …/svd/{peripheral}?read=true`, `PUT
  …/svd/{peripheral}/{register}`) refuse agents: reading memory-mapped registers has side
  effects, so only you do it.

## A project in a dev container

A project whose terminals and runs use its dev container builds in it: the toolchain is
there. The probe is plugged into this computer, so the debugger and the debug server stay
here. For a remote target Workbench therefore runs `pre_launch` (your build) **in the
container**, and gdb and the server on this computer, and adds `set substitute-path` for
the workspace so the paths the container recorded in the ELF (`/workspaces/app/src/…`) find
the files here. The configuration's tooltip and the Console say so. Other `source_map` pairs
(a toolchain's sources, a CI path) go in the table as above. The built ELF must be
somewhere the container shares with this computer: the workspace.

## Choosing the GDB

If the configuration names no `adapter`, Workbench reads the program's ELF header and
picks: for an Arm program `arm-none-eabi-gdb`, then `gdb-multiarch`; RISC-V `riscv-gdb`
(`riscv32-unknown-elf-gdb`, `riscv64-unknown-elf-gdb`, `riscv-none-elf-gdb`,
`riscv32-esp-elf-gdb`), then `gdb-multiarch`; Xtensa `xtensa-gdb` (`xtensa-esp32-elf-gdb`,
`xtensa-esp32s3-elf-gdb`, `xtensa-esp-elf-gdb`…), then `gdb-multiarch`; the plain `gdb` only
for the computer's own architecture. The first one that is available wins. A program that
the pre-launch step has not built yet is not known: `gdb-multiarch` comes first, so name an
`adapter` if you have several GDBs and no multi-architecture one.

To use a GDB that is not on `PATH`, or to change gdb's arguments, edit the preset or define
your own in `config.toml`:

```toml
[debug.adapters.arm-none-eabi-gdb]
command = "/opt/arm-gnu-toolchain/bin/arm-none-eabi-gdb"

[debug.adapters.my-gdb]               # any other gdb, named by `adapter = "my-gdb"`
kind = "gdb"
command = "~/tools/riscv/bin/riscv32-unknown-elf-gdb"
args = ["-q", "-i", "dap"]

[debug]
default_adapter.embedded = "my-gdb"   # the GDB for every remote target
```

## Your own debug server

Servers are commands, so only `config.toml` defines them, like debug adapters. A
repository's configuration names one by id and adds arguments:

```toml
[debug.servers.openocd]               # change a preset: only what you set changes
command = "~/xpacks/openocd/bin/openocd"
reset = ["monitor reset init"]

[debug.servers.gdbserver]             # or add one: gdbserver runs the program itself
label = "gdbserver"
command = "gdbserver"
args = ["--once", "127.0.0.1:{port}", "{program}"]
download = false
ready_timeout_s = 60
```

A configuration with `server = "gdbserver"` and the program's path then debugs it through
gdbserver, the way a remote Linux target is debugged.

`command`, `args`, `label`, `env`, `enabled`, `init`, `reset`, `download`, `ready_timeout_s`
(how long to wait for the port, default 30) and `install_hint` are the fields. A server
must listen on `{port}` (or on the `port` or loopback `connect` of the configuration):
that is how Workbench knows it is ready. It waits for the port without connecting to it,
because servers such as `st-util`, `gdbserver --once` and pyOCD serve a single connection
and exit.

## While it runs

- The **Console** shows the server's own output in italics (OpenOCD's `Info :` lines,
  QEMU's serial port) between your commands and the program's output. The commands the
  preparation runs, and `load`'s progress, are echoed like commands you typed.
- Type gdb commands in the console: `monitor reset halt`, `monitor info registers`,
  `info registers`, `x/16x $sp`. Tab completes.
- **Variables** shows the frame's arguments and locals, **Registers** (core registers, and
  the Cortex-M `msp`, `psp`, `primask`, `control`…) and **Globals**. **Pause** halts a
  running core; **Resume**, the step buttons and *Run to Cursor* work as in any session.
- The header shows the server and where gdb is connected (`OpenOCD · 127.0.0.1:28329`).
- **Ask agent about this stop** hands the stop, the stack and the variables to an agent;
  an agent can also read and steer the session itself ([Agents](#agents)).
- **Rerun** stops everything and starts it again: the build, the server (on new ports),
  the download. **Stop** disconnects gdb and ends the server; what the chip does then is up
  to the server and the target.

## Agents

An agent can see and steer the session you started; it cannot start one.

- `debug_state` reads it: where it stopped, the stack, the locals, the CPU registers (with
  `registers`) and the console's tail, including the server's and the channels' output.
- `debug_control` continues, pauses, steps (`next`, `stepIn`, `stepOut`), runs to a line or
  stops the session, and answers with the new state, so a loop "step, look, step" is one
  call each. It waits for the program to stop again (`waitSeconds`, default 15).
- `debug_breakpoints` lists, adds, removes, sets, mutes and clears the project's plain line
  and function breakpoints; the editor shows them at once.
- These are writes: the agent's own permission prompt applies, the Activity view marks
  them, and they act only on the agent's own project. Starting, attaching, rerunning,
  **evaluating expressions** and **conditional breakpoints or log points** are not offered
  to agents: a gdb expression can run a shell command (`$_shell(...)`), and a start runs a
  configuration's build step and debug server. You do those from the window.

## Troubleshooting

- *"OpenOCD exited before it was ready (exit code 1): Error: unable to find a matching
  CMSIS-DAP device"*: the server's last error lines are in the message and its whole output
  in the console. Check the cable, the udev rule, the interface and target files.
- *"… did not listen on port … within 30s"*: the server started but never opened its gdb
  port (a target that does not answer, a script that sets another `gdb_port`). Name that
  port in `port`, or give a slow server more time with `ready_timeout_s`.
- *"port 3333 is already in use"*: a `port` you fixed is taken, often by an OpenOCD from
  an earlier run. Stop it, or drop `port` and let Workbench pick one.
- *"gdb could not connect to …"*: the server listens but gdb cannot talk to it; the
  console has both sides' words.
- *"this GDB has no DAP support: GDB 14 or newer is needed"* or *"built without Python"*:
  install `gdb-multiarch`, or a toolchain whose GDB is 14 or newer with Python
  (`arm-none-eabi-gdb-py` is picked first when it exists).
- *A breakpoint stays hollow*: the program's symbols did not load (build it first, or add
  `pre_launch`), or the line has no code.
- *"`load` failed"*: the console shows the server's reason (a locked chip, a wrong flash
  driver). Set `download = false` to debug what is on the chip already.

## Limits

**Verified** with real tools: QEMU's Cortex-M3 (launch, breakpoints, the reset vector, its
UART as a channel, SysTick through the Peripherals tab), `gdbserver` and `gdbserver --multi`
(`extended`), the parser against ST's STM32F407 SVD, a dev container (a throwaway Debian
container, a firmware built with container paths), and `source_map`.

**Not verified** (written from the tools' documentation and tested against stand-ins):

- OpenOCD, J-Link, pyOCD and `st-util` driving a chip, J-Link's RTT and SWO ports, and the
  ITM decoder against a real SWO stream.
- `attach` on an extended-remote stub (Black Magic Probe): with GDB 17.1 the DAP server
  aborted when asked to attach to an `extended-remote` target in my test, so only the
  launch form (`gdbserver --multi`) is known to work.
- A dev container with a real cross toolchain inside: the build step and the path mapping
  were exercised, the compiler was not.
- Windows is experimental: the code paths are type-checked there, not exercised.

**Not supported:**

- RTOS-aware thread lists come from the server (OpenOCD's `-rtos`, J-Link's RTOS plugins),
  through gdb's own thread list; Workbench adds nothing to them.
- A debug server that runs inside the container.

## Safety

A server's command comes only from `config.toml` or a preset, never from a repository. A
repository's configuration can add arguments, a `connect` address and gdb commands; like
`pre_launch` they run only when you start that configuration, and the Start view shows
them first. Nothing starts by itself. Agents steer a session you started and cannot start,
attach, evaluate expressions or set conditions ([Agents](#agents)); the register map's
routes are closed to them.
