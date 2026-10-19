# Live Watch and plots: variables of running firmware

Read this when the user wants to see how variables of running firmware behave over time without stopping it.
Only the user sets Live Watch up. You read what it records.

## What it is

The Debug window's Live tab shows variables of the program while it runs. The user types a variable (`ticks`,
`cfg.limit`, `buf[3]`) or a fixed address (`*(uint32_t*)0x50000014`) and presses Enter; its value is read
every 250 ms (50 ms to 5 s) and drawn as a line. The Plots button draws several watched values together on one
chart: up to 8 series per plot, a span from 5 seconds to 30 minutes, one axis in the values' own unit
(`shared`) or every series as a percentage of its own range (`normalized`). Plots and watched expressions
belong to the project, so the next session watches them again (one that no longer resolves after a rebuild
stays in the list with the reason).

There is no tool to add a watch, change one or make a plot. If nothing is watched, ask the user to type the
variables into the Live tab and to choose Plots, New Plot from Watched Values.

## When a session has it

- Only a gdb session on an embedded target (a configuration with `[debug.remote]`) has a live view. A host
  program has none, and `debug_plots` then carries a note that no session can read values.
- With OpenOCD's Tcl port (the OpenOCD preset has it; another OpenOCD needs `live_port` in config.toml) memory
  is read over the debug link while the core runs, and the program is never stopped.
- With J-Link, pyOCD, `st-util`, QEMU or an OpenOCD without a Tcl port the values stay blank until the user
  allows "reading by pausing the program". Each read then stops the program for a few milliseconds (every 100
  ms or slower) and resumes it. Code that depends on timing (motor control, communication, watchdogs) can
  notice; the Live tab shows the cost (on one board through OpenOCD about 50 ms a read, 17 percent of the time
  at 250 ms). It is off for every new session and is the user's decision. If `live` values are blank, say so
  and let the user decide; do not ask for it lightly.

## What can be watched

- Things at a fixed address: a global, a member, an element, a fixed address; up to 64 bytes each, 16 at once.
  Integers, floats, booleans, pointers and enums are shown by their type; arrays and structs as bytes and are
  not drawn.
- A pointer's target moves, so `*ptr`, `p->x` and `buf[i]` are refused with the reason: watch the pointer
  itself or the address.
- Peripheral registers are memory too, and reading some changes the chip. Registers the project's SVD file
  marks as changing on read are refused by name; other addresses in the peripheral regions carry a warning.
- Readings are not snapshots: a 64-bit value, or two values read one after the other, can tear while the
  program runs. Values are little-endian. A value that cannot be read shows the debug server's message and the
  others go on.

## What you read

- `debug_state`: each session lists `live`, the watched expressions with `expression`, `type`, `value` (the
  latest reading) and `error`.
- `debug_plots` with no arguments lists the plots: `id`, `name`, `series` (the expressions), `windowMs`,
  `scale`. With `plot` (id or name) it returns the data: `seconds` back (default the plot's span, at most
  3600) and `maxPoints` readings per series (default 120, at most 1000). Per series: `expression`, `watched`
  (false: that session does not read it), `type`, `stats` (`readings`, `failed`, `min`, `max`, `mean`, `last`)
  and the readings as `t` (milliseconds since the epoch) and `v` (`null` is a failed reading, a gap).
  `intervalMs` is the polling period and `until` the newest reading. The thinning keeps each slice's lowest
  and highest, so a spike survives. Readings come from the project's newest session that can read values;
  Workbench keeps the newest 12,000 per value (about ten minutes at 50 ms, fifty minutes at 250 ms).
- A `note` in the answer says why there is nothing: no session can read while the program runs, or a series is
  not watched.

Use the statistics to answer questions such as: did the counter stall (`min` equals `max`, or `last` stopped
changing), did the flag keep toggling (`min` 0 and `max` 1 with many readings), did a sensor drift (compare
early and late readings), how many reads failed (`failed`).

## When it is not available

Stop the program and look at one moment: `debug_control` with `pause`, `debug_evaluate` for the values, then
`continue`. That is a snapshot, not a trace, and it stops the firmware. For numbers the firmware itself
prints, a UART, RTT or SWO channel in the configuration puts that text into the console, which `debug_state`
returns.
