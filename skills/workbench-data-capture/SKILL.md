---
name: workbench-data-capture
description: >-
  Puts measurements into Workbench's live data viewer while a capture runs and reads statistics back: logic analyzer lines, probe or scope waveforms, power profiles, any value, level or note over time. Use when, in a Workbench session, the user asks to capture, record, monitor, plot or measure signals, current, voltage, power or timing, or to look at a CSV, VCD or JSON-lines capture. Not for variables of a program under a debugger (workbench-debug-session).
license: MIT
compatibility: Needs Workbench's MCP tools (a Claude Code or Codex session started from Workbench, version 0.16 or later). Plots show in a desktop browser. Checked against Workbench 0.16.0.
---

# Capturing measurements in Workbench's data viewer

This skill applies only when the `workbench` server's `data_append`, `data_series`, `data_plot`, `data_follow`
and `data_query` tools are in your tool list; if they are not, ignore it and use ordinary tools. In Claude
Code they are named `mcp__workbench__data_append` and so on, and a tool that seems missing may only be
deferred: load it with the tool-search facility before concluding it is unavailable.

Workbench talks to no hardware. You run the capture tool (sigrok-cli, a vendor CLI, a PyVISA or DAQ script);
Workbench keeps the samples in memory, draws them beside the session while they arrive and answers questions
about them.

## When to use the viewer

- For a value, a level or a note over time that you capture while a tool runs and the user may want to watch,
  zoom into or measure: current, rails, probe waveforms, logic lines, decoded bytes, "radio on" markers.
- To answer a question about a capture, call `data_query` instead of printing or reading raw samples: min,
  max, mean, RMS, area, edges, duty and pulse widths are exact over the window you give, and samples come back
  thinned.
- Not for one number or a small table (put it in your answer), text logs, or anything without a time axis.
- Plots show on desktop only. If you cannot tell that the user watches a desktop browser, give the key numbers
  in text as well.

## Workflow

1. **Define the series first.** `data_series` with `action: "define"`, `series`, `kind` (`analog`, `digital`
   or `events`), `unit` (the unit of the numbers you will send: `A`, `V`, `W`) and `group` (a heading per run,
   such as "power run 3"). A plot picks lanes and colours from what the project knows about each series when
   it is created; an unknown name is treated as analog.
2. **Make the plot early**, so it fills live. `data_plot` with `action: "create"`, `title`, `series` and
   `span` (seconds in view while it follows the newest samples; default 10, `0` shows everything captured).
   Lanes are chosen for you: analog series with the same unit share one, each digital line and each events
   series gets its own. Pass `lanes` instead for `log: true` (current that spans decades), fixed `min` and
   `max`, or `weight`; `markers` draws events series as vertical lines. The plot opens in the user's window
   and may name series that have no samples yet. With `data_follow` the series appear when the first rows
   arrive: define them yourself first if you know the header (and any `prefix`), or create the plot once
   `data_series` lists them.
3. **Feed samples** (next section) and start the capture so data arrives while the plot is open. Change the
   plot as the capture goes with `data_plot` `action: "update"`, `plot` (id or title) and `add` or `remove`;
   passing `series` again rebuilds the lanes and drops your lane settings.
4. **Check that it flows.** `data_series` with `action: "list"` shows `count`, `tEnd` and `dropped` per
   series; `data_follow` with `action: "list"` shows each source's `state`, `rows`, `skipped` and `error`.
5. **Read it back.** `data_query` with `series` (or `plot`) and a window: `from` and `to` in seconds, or
   `last` (seconds before the newest sample); `maxPoints: 0` returns statistics only. Ask it for the sleep
   current, a pulse width, whether a rail dipped.
6. **Hand over.** The plot is already open in the user's window: give its title and what it shows, the answer
   as numbers with units and the window you measured, and the caveats (`dropped`, `skipped`, failed readings,
   a capture cut short). Say where the raw data is, stop what you started (`data_follow` with `action: "stop"`
   and the source `id`, background processes), and for a written deliverable use the `workbench-report-card`
   skill.

## Feeding data: choose by who produces the numbers

| Situation | Use |
| --- | --- |
| A few values, markers or notes you already hold | `data_append` |
| Your own script reads an instrument or a serial port | the script posts to the hook with the session's token |
| A capture tool writes a CSV, JSON-lines or VCD file, or you have a finished file | `data_follow` |

Never type hundreds of samples into a tool call: they pass through your context. Let a script or a file carry
them.

- `data_append`: one item per series, `batch` for several (at most 256 items). On a series' first append of
  `values` give `t0` (usually `0`): without it the samples are stamped with the clock. The accepted shapes are
  in `references/tools-and-limits.md`.
- The hook is `POST $WORKBENCH_URL/api/hooks/data/$WORKBENCH_TERMINAL_ID/append` with `Authorization: Bearer`
  and the session's token from `WORKBENCH_AGENT_TOKEN`, and a `data_append` body. The token stays in the
  environment: never type, echo or log it. Copy-paste forms are in `references/feeding-data.md`. If the
  variables are empty, or the post cannot connect (a sandboxed shell), write the samples to a file and use
  `data_follow`.
- `data_follow`: `path` inside the project, in a normal folder such as `captures/` (a file that does not exist
  yet is waited for up to a minute). It answers at once and reads in the background; `follow: false` imports a
  finished file, so wait for `state: "done"` in `data_follow` with `action: "list"` before you query. Give
  `timeColumn`, `timeUnit` or `rate` when the file has no time column the viewer recognises, and `digital` for
  logic columns.

## Rules and limits that are easy to get wrong

- Times are seconds everywhere (`t`, `t0`, `dt`, `from`, `to`, `last`, `span`, `offset`); `rate` is samples
  per second. A file's time column is read as seconds unless its header says otherwise (`Time (ms)`) or you
  pass `timeUnit`. Times above 1e8 are read as Unix seconds (a clock): do not mix them with times counted from
  zero in one plot. `offset` shifts a series to line instruments up.
- Times never go backwards within a series (equal is fine). A new capture starts with `clear: true` on its
  first append, or `data_series` with `action: "clear"` and `names` or `group`. A refused request changes
  nothing.
- A series keeps its kind: events into an analog series, or numbers into an events one, are refused, and
  `define` with another kind empties the series. `digital` keeps transitions only, and a value above 1 is a
  bus. `null` in `v` is a failed reading (a gap).
- `unit` must describe the numbers you send: `0.0052` with `A` shows as 5.2 mA, a milliamp value declared as
  `A` is wrong by a thousand. SI prefixes are applied to units such as `A V W Hz s Ω F J C`, with or without a
  prefix (`uA`, `mA`); other units (`°C`, `%`) show as given. Series that share a lane need the same unit
  string: nothing is converted between traces.
- Memory: a series keeps its newest `maxPoints` samples (default 1,000,000, at most 10,000,000; events 20,000)
  and 30 million are held across all projects; older samples roll off. Samples are not saved: after a
  Workbench restart the series definitions and plots remain, empty. Keep the capture file if the data matters.
- Names are 1 to 64 characters, `unit` at most 16, `group` at most 64, event text 200. Per project: 256
  series, 40 plots, 32 lanes in a plot, 8 traces in a lane.
- `data_follow` reads only files inside the project, never hidden paths (`.cache/run.csv`), key-like names
  (`*.pem`, `*.key`, `*_token`), files the project marks sensitive, or links that lead out. At most 8 sources
  run, and one file once. A file that shrinks or is replaced starts the capture over and clears its series.
  Rows whose time goes backwards are skipped and counted.

## Mistakes to avoid

- Creating the plot before defining the series: a digital line then gets an analog lane.
- Leaving `span` at 10 for a long capture: the plot shows only the last ten seconds.
- Calling `data_plot` with `create` twice for one plot: it makes a second plot. List first.
- Following a file that has no time column and no `rate`: rows are stamped when Workbench reads them, not when
  the instrument sampled (a finished import counts rows as seconds).
- Mixing up `maxPoints`: in `data_append` and `data_series` it is how many samples are kept, in `data_query`
  how many come back.
- Treating statistics as covering the whole capture after it rolled over or while it still runs.
- Leaving followed files and capture processes running when the work is done.

## Reading more

- [references/feeding-data.md](references/feeding-data.md): read before you write a script that posts samples
  (curl and Python, token handling) or follow a file (formats, columns, what is refused, how to read a
  source's state).
- [references/tools-and-limits.md](references/tools-and-limits.md): read for exact `data_append` shapes and
  refusals, plot and lane fields, what `data_query` returns and every limit.
