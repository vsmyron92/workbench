# Data viewer: live measurements next to the agent

The **Data** tool window and its plots show measurements while an agent captures them:
the output of a logic analyzer, a differential probe or oscilloscope, a power profiler,
or anything else that is a value, a level or a note over time. The agent puts samples in
and makes the plots; they open beside its session and fill live, so you can watch a
capture, steer it ("zoom into that wake-up", "plot the SPI lines too") and keep asking
questions of the same data.

Nothing here talks to hardware by itself. The agent runs your capture tool (sigrok-cli,
a vendor CLI, a Python script with PyVISA or a DAQ library) and gives Workbench the
numbers. Workbench stores them, draws them and answers questions about them.

## What a plot shows

A **series** is one named signal:

| Kind | What it holds | Drawn as |
| --- | --- | --- |
| `analog` | numbers over time: a current, a rail voltage, a probe waveform | a line (or steps, or dots) |
| `digital` | levels over time; only transitions are kept, so a fast capture of a quiet line costs nothing; values above 1 are a bus | a step line, or boxes with the value in hex |
| `events` | text at a time: a decoded byte, a trigger, "radio on" | ticks with their text |

A **plot** is a few **lanes** stacked under one shared time axis, like a scope or a logic
analyzer's rows. Each lane has its own vertical axis and holds the traces that share it:
power in µA wants its own (logarithmic) lane, a rail voltage another, each digital line a
thin one, decoded events a row of labels. Analog traces take the chart colours (eight);
digital lanes and events are named at their left edge and drawn in the text colour, so a
16-channel capture is no problem.

Values are written with SI prefixes for their unit (`0.0052 A` is "5.2 mA", `5200 uA` too),
and times in the unit that suits the scale (2 ms, 0.5 µs) or as a clock when the times are
Unix seconds.

## In the plot

| Do | To |
| --- | --- |
| **Wheel** | zoom in time around the pointer |
| **Drag** | pan; the plot stops following |
| **Shift + drag** | measure a range: min, max, mean, RMS, peak-to-peak and the area under each analog trace (charge for amps, energy for watts); frequency, duty and edges for digital lines |
| **Double-click**, **End**, or **Following** | follow the newest samples again |
| **F** | fit everything captured |
| **+ / −**, **← / →**, **Home** | zoom, pan, jump to the oldest sample |
| **Esc** | clear the measured range |
| lane's **⋯** | logarithmic scale, fixed range, rename, move, remove |
| legend | click a name to hide or show a trace; the value under the cursor is beside it |
| **+** in the bar | add a series; **Save as CSV** exports what is in view |
| the chain button | **linked** plots pan and zoom together (side by side in the dock), so a power plot and a logic plot stay aligned |

The bar under the plot is an overview of the whole capture; drag it to move through time.

## What the agent does

Agents see these tools (they are listed under **Activity** as they are used):

| Tool | Does |
| --- | --- |
| `data_append` | puts samples or events into a series, creating it; several series per call |
| `data_series` | lists series; defines unit, kind, group, offset; clears or deletes |
| `data_plot` | creates, changes, opens and deletes plots; `add` / `remove` series while the capture runs |
| `data_follow` | reads a CSV, JSON-lines or VCD file **while a capture tool is still writing it**, or imports a finished one |
| `data_query` | reads back statistics (min, max, mean, RMS, percentiles, area; edges, frequency, duty, pulse widths) and thinned samples of a window |

A typical session: the agent starts the capture tool writing to `captures/run7.csv`, calls
`data_follow` on that file and `data_plot create` with the series it expects (a plot may be
made before its series have a sample), and the plot opens beside the session and fills. It
changes the plot as the capture goes (`log: true` on the current lane, a marker series for
"wake" and "TX"), and when the capture ends asks `data_query` for the sleep current and the
width of the wake pulse.

For a script the agent writes itself, there is a hook with the session's own token:

```bash
curl -s -X POST "$WORKBENCH_URL/api/hooks/data/$WORKBENCH_TERMINAL_ID/append" \
  -H "Authorization: Bearer $WORKBENCH_AGENT_TOKEN" -H 'Content-Type: application/json' \
  -d '{"series":"VDD","unit":"V","values":[3.30,3.29,3.31],"t0":0,"dt":0.001}'
```

Samples are `{series, t: [...], v: [...]}`, `{series, values: [...], t0, dt}` (or `rate`), or
one `{series, v}` stamped now. `null` in `v` is a failed reading. `{batch: [...]}` carries
several series in one request.

### Following a file

| Format | Notes |
| --- | --- |
| `csv` | comma, semicolon, tab or white space; a header line names the columns, and `Current [uA]` or `Time (ms)` give the unit; a column called time, t or timestamp is the time (seconds, or `timeUnit`; dates work); with no time column give `rate` or rows are stamped as they arrive; text cells become events |
| `jsonl` | one JSON object per line: `{"t": 0.5, "i": 1.5, "state": "RX"}` |
| `vcd` | logic analyzer and simulator waveforms (sigrok, PulseView, GHDL, simulators): signals become digital series, buses numbers, scopes name them |

A file is read only when it is inside the project, not hidden, not key-like and not
marked sensitive (the same rule as every agent file read); symbolic links are resolved
first, and again at every look, so a file that becomes a link later is refused. A file
that shrinks or is replaced starts the capture over. A column that is numbers with an
occasional word (an instrument's `OVLD`) stays a series of numbers; the words go to a
series named `<column> (text)`. Rows whose time goes backwards are skipped and counted
in the Data window, never fatal.

## Limits

- **Samples are kept in memory, newest 1,000,000 per series by default** (up to 10 million;
  `maxPoints`), 30 million across all projects. A capture left running rolls. What survives
  a restart is the series' definitions (names, units, groups) and the plots, not the samples:
  keep the file the capture wrote if you want the data.
- Times must not go backwards within a series: a new capture starts from an empty series
  (`clear: true`, or **Clear** in the Data window). A request that is refused changes
  nothing, and a series keeps its kind: events into an analog series are refused, not
  converted.
- A plot is drawn from what the server returns for the window in view (a few thousand
  points, keeping each pixel column's lowest and highest), so a spike survives however much
  data there is, and a reload or a second browser sees the same plot.
- 256 series and 40 plots per project; 32 lanes and 64 traces per plot.
- Desktop only for now: the phone has no plot panel.
