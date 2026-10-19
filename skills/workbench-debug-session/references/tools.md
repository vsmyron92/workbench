# Debug tools: parameters, answers and errors

Parameter names are exactly those of the tools' JSON Schemas.

## Parameters several tools share

- `projectId`: `debug_start`, `debug_attach`, `debug_restart`, `debug_control`, `debug_evaluate` and
  `debug_breakpoints` take any project's id; `debug_state` and `debug_plots` only your own. Default: your
  session's project. Leave it out (see Trust and safety in `SKILL.md`).
- `sessionId`: the session to act on. Without it the tool uses the project's only live session; with none it
  answers that there is no live session (use `debug_start`), with several it names them and asks for one.
  `debug_state` without it lists every session of the project.
- `waitSeconds`: 0 to 300. Defaults: 60 for `debug_start`, `debug_attach`, `debug_restart`; 15 for
  `debug_control`.
- `frame` and `registers`: `frame` is the stack frame whose locals the answer lists (0 is the innermost, at
  most 29); `registers: true` adds that frame's CPU registers.

## What an answer holds

`debug_state` answers `{projectId, sessions: [..]}`, or a text line when the project has no session. The tools
that wait (`debug_start`, `debug_attach`, `debug_restart`, `debug_control`) answer for one session with the
same fields plus `settled`. Per session:

- Always: `sessionId`, `name` (the configuration), `adapter`, `request` (`launch` or `attach`), `state`
  (`starting`, `running`, `stopped`, `terminated`, `failed`), `error`, `exitCode`, `process`, and for a remote
  target `remote: {server, target}`.
- `console`: the newest 4000 characters of the last 40 console entries. It includes the debug adapter's
  output, the debug server's own lines (OpenOCD's `Info :` lines, QEMU's serial port), the text of
  `[[debug.remote.channels]]`, the commands the start-up ran and `repl` commands with their answers. It is
  what explains a failed start or a crash. The output of a launched native program is not here when it runs in
  a terminal tab of its own (the default where the adapter supports it); the configuration's
  `console = "console"` sends it here.
- `live`: the Live Watch expressions of the session with `expression`, `type`, `value` and `error` (see
  `live-watch.md`), when it has any.
- Only while `stopped`: `stop: {reason, description, text, threadId}`, `threads` (up to 40), `stack` (up to 30
  entries, `#0 function at file:line`), `frame`, `locals` (a list of
  `{scope, variables: [{name, type, value}]}`: locals and arguments one level deep, 60 values at most, each
  cut at 300 characters; registers, globals, statics and scopes the adapter calls expensive are left out),
  `registers` when asked (`{name: value}`, 64 at most, each cut at 80 characters), `stackError` when the stack
  could not be read. Secret values are masked.
- `settled` (the waiting tools only): `true` when the session is stopped, ended or failed; `false` when the
  wait ran out while it was still running or starting.

## debug_start

`config` (required): the exact name of a launch configuration of the project. `stopOnEntry`: overrides the
configuration (for gdb: stop at `main`). `pid`: the process the user named, for an attach configuration that names none
(without it such a configuration answers `pid_required`). Runs, in order: the build step (`pre_launch`), the
debug server for a remote target, the adapter, the connection, then for a remote target the preparation
(initialise, reset, download, reset, run to the first stop). It returns when the session settles or
`waitSeconds` is over.

## debug_attach

`pid` (required; pid 1 and Workbench's own process are refused), `adapter` (an adapter id; default guessed),
`language`, `program` (the executable, for symbols). Native attach goes through ptrace, which the system may
restrict. It is refused for a project that runs in its dev container.

## debug_restart

`sessionId`. Only for a session that came from a launch configuration; one started by `debug_attach` has none
and is refused ("only sessions of a launch configuration can be rerun"): attach again. The configuration is
planned again first (if it no longer works, the old session stays), then the session is stopped and forgotten,
the build step runs again and a new session starts; the answer carries the new `sessionId`.

## debug_control

`action` (required): `continue`, `pause`, `next` (step over), `stepIn`, `stepOut`, `runTo` (`path`
project-relative and `line` 1-based, both required), `stop`. `threadId`: the thread to step (default: the one
that stopped). All but `stop` wait up to `waitSeconds` for the program to stop again; the answer says
`settled`. `stop` ends the session: a launched program is terminated, an attach or a microcontroller is
detached; for a remote target Workbench then ends the debug server too.

## debug_evaluate

`expression` (required, at most 10,000 bytes), `context` (`watch`, the default, or `repl`), `frame`,
`threadId`, `sessionId`. The session must be stopped; a running program makes the debugger refuse. It gives up
after 30 seconds. The answer is
`{sessionId, expression, context, result: {name, value, type, variablesReference, memoryReference, ..}}`;
`value` is cut at 16 KiB. A `variablesReference` above 0 means members exist but no tool expands them:
evaluate `x.member` or `x[3]`, or use `print` in `repl`.

- `watch`: an expression of the program's language in the frame. It can assign and call functions, so it can
  change the program.
- `repl`: a debugger console command. The command and its answer appear in the user's console as
  `> command   (agent)`. With gdb: `info registers`, `info registers pc`, `x/16x $sp`, `bt`, `info threads`;
  on a board `monitor reset halt` and other server commands (they act on the real target: only what the user asked for).

## debug_breakpoints

`action` (required): `list`; `add` (lines of `path`, keeping the file's others; a new breakpoint replaces one
on the same line); `remove` (`path` and `lines`; without `lines` every breakpoint of the file); `set`
(replaces the file's breakpoints); `functions` (replaces the function breakpoints:
`{name, enabled, condition}`); `mute` (`muted`, default true: sessions get no breakpoints while muted);
`clear` (every breakpoint of the project).

- A line breakpoint is `{line, enabled, condition, hitCondition, logMessage}`. `path` is project-relative with
  `/`, no leading `/` and no `..`. `condition` is an expression the debugger evaluates; `hitCondition` goes to
  the adapter as written; `logMessage` is printed with `{expression}` interpolated and does not stop. Each
  text is cut at 2,000 bytes. One breakpoint per line; 300 per file, 2,000 per project, 200 function
  breakpoints.
- The answer lists `breakpoints` and `functionBreakpoints`, each with `status: {verified, line, message}`
  (`null` when no session is live; `verified: false` with the adapter's message when gdb could not place it,
  for example before the symbols loaded or on a line with no code), plus `exceptionFilters`, `muted`,
  `watches` and `live` (whether a session is live).
- Breakpoints persist with the project and are the user's too: the editor shows them, every session gets them.

## debug_plots

`plot` (an id or a name; without it the list: `id`, `name`, `series` as the expressions drawn, `windowMs`,
`scale`), `seconds` (how far back; default the plot's span, 1 to 3600), `maxPoints` (readings per series;
default 120, 4 to 1000). With a plot it answers `plot`, `sessionId`, `intervalMs`, `until`, `seconds`, and per
series `expression`, `slot`, `hidden`, `watched`, `type`, `stats` (`readings`, `failed`, `min`, `max`, `mean`,
`last`) and the readings as `t` (milliseconds since the epoch) and `v` (`null`: a failed reading). A `note`
says when no session reads values or a series is not watched. See `live-watch.md`.

## Errors you will meet

| Message (shortened) | Meaning and what to do |
| --- | --- |
| no launch configuration "X" in the project | The name is wrong or the entry is in a file you cannot see. Read `.workbench.toml`; ask the user. |
| "X" attaches to a process: pick one (`pid_required`) | An attach configuration: pass the `pid` of the process the user named, and ask which when it is unclear. |
| the debug adapter is not installed or too old (`not_configured`, with an install hint); an unknown debug server id (`not_configured`) | gdb older than 14, or built without Python, or a server id that no preset or config.toml defines. Tell the user what the hint says; do not install software yourself. |
| a failed session whose `error` says a server was not found on `PATH` | The debug server's command is not installed. Same: tell the user. |
| the pre-launch run deploys or reaches another host | Refused for everyone. The user starts that run themselves, then debugs without `pre_launch`. |
| no live debug session in this project | Start one with `debug_start`, or the session already ended. |
| N live debug sessions: name one with sessionId | Pass `sessionId` from the list in the message. |
| the program is not suspended / the program is not running | `continue`, the steps and `runTo` need a stopped session, `pause` a running one. Look at `state` with `debug_state` first. |
| at most 16 debug sessions can run at once | Stop one first. |
| pick another process | `pid` was 1 or Workbench's own. |
| an adapter message (status 422, `debugger_error`) | The debugger refused: for evaluate usually because the program is running; read the text. |
| this session has no live view | Live Watch needs a gdb session on an embedded target and a debug server it can read through (see `live-watch.md`). |
