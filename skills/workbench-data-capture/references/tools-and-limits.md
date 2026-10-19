# Data tools: shapes, fields and limits

Parameter names are exactly those of the tools' JSON Schemas. Every tool has an optional `projectId`: leave it
out. It defaults to your session's project, and naming another project is refused.

## data_append

Samples or events for one series (created by its first append), or several series in `batch`. One shape per
request and series:

| Body | Meaning |
| --- | --- |
| `{series, t: [..], v: [..]}` | a time per value; the lists have the same length |
| `{series, points: [[t, v], ..]}` | the same, as pairs |
| `{series, values: [..], t0, dt}` | even spacing from `t0`; `rate` (samples per second) instead of `dt` |
| `{series, values: [..]}` | continues after the series' newest time, spaced by the request's `dt` or the series' `rate` |
| `{series, v}` | one sample; with `t` at that time, without it now (the clock) |
| `{series, events: [{t, text}, ..]}` or `{series, text, t}` | events; without `t` now, never before the newest event |

Next to the samples a request may carry `kind`, `unit`, `label`, `group`, `offset`, `rate`, `maxPoints` and
`clear`. `clear: true` empties the series first (a new capture). A `rate` stays with the series.

Values are numbers; `true` and `false` (or `"high"`, `"low"`) are 1 and 0; `null` and `"nan"` are failed
readings, as is an infinite value. `unit` is plain text; `label` says what is measured and where the probe
sits.

Refused, with nothing changed: samples and events in one request; a list of times next to one value, or one
`t` next to a list of values (give a list, or `t0`); several values with no times and neither `dt` nor a
`rate`; `dt` of zero or less; a time that is not a number or goes backwards; more than 2,000,000 samples;
events into a numeric series or numbers into an events one; a new series beyond 256. A new series sent
`events` or `text` becomes an events series. In a `batch` every item is read before any is applied; then they
run in order, and a series that refuses one stops the batch there (earlier items stay, the error names the
item).

The answer lists, per series, `+taken (now N held, newest at T s)`, `[new series]` and
`[K oldest rolled off]`.

## data_series

- `list` (the default): every series with `name`, `kind`, `unit`, `label`, `group`, `offset`, `maxPoints`,
  `rate`, `count`, `appended`, `dropped`, `tStart`, `tEnd`, `wall` (true: times are Unix seconds) and `last`;
  also the plots and the followed files. `tStart` and `tEnd` include the `offset`.
- `define`: `series` plus any of `kind`, `unit`, `label`, `group`, `offset`, `rate`, `maxPoints`, without
  samples. A new `kind` empties the series; a lower `maxPoints` drops the oldest samples; `rate` of 0 forgets
  the rate.
- `clear`: `names` or `group`; with neither it is refused (clearing every series is never the default). The
  definition stays.
- `delete`: `names`.

## data_plot

- `create`: `title`, then `series` (names; lanes are made) or `lanes`, with `span`, `follow`, `linked`,
  `markers` and `open`. Without series or lanes it is refused. The plot opens in the user's window unless
  `open: false`; `action: "open"` opens it again. A second `create` with the same title makes a plot with the
  id suffixed `-2`; use `action: "list"` first.
- `update`: `plot` (id or title) and any field above; `add` and `remove` change the series and keep the lane
  settings; `series` or `lanes` replace the lanes.
- `delete`: `plot`. `list`: the plots with their lanes.
- A lane is `{title, series: [..], min, max, log, weight}`. `min` must be below `max`; `log` needs a `min`
  above 0; `weight` is 0 (by kind: analog 3, digital 1, events 1) or 0.2 to 20. The lane's axis takes the unit
  of its first analog trace that has one; values of the other traces are drawn as they are.
- Lanes made from `series` alone: analog series with the same unit string share a lane (8 at most), an analog
  series without a unit has a lane of its own, each digital and each events series has its own; analog lanes
  first, then digital, then events. A name the project does not know yet counts as analog.
- Analog traces get the chart colours 1 to 8 once, when the plot is saved (more than eight repeat); digital
  and events traces are drawn in the text colour, so any number of logic lines is fine.
- `span`: `0` (everything captured) or 0.000001 to 86400 seconds, used while the plot follows. `follow: false`
  leaves the view where the user put it. `linked` (default true) pans and zooms together with the project's
  other linked plots.
- A plot's title is at most 60 characters. A series is in a plot once; `markers` are events series, at most 8.

## data_query

Arguments: `series` (names) or `plot`, `from`, `to`, `last`, `maxPoints` (samples returned per series: default
200, at most 2000, `0` for statistics only), `events` (default 100, at most 500). It reads only.

- The window is `from` and `to` in the series' own times, `offset` included; `last` counts back from the
  newest sample of the named series (with `to` it counts back from `to`). For a series that uses the clock,
  prefer `last`.
- The answer is
  `{projectId, window, series: {name: {kind, unit, label, count, tStart, tEnd, last, stats, t, v, samplesInWindow, thinned}}}`.
  Events series carry `events` (the newest ones as `[t, text]`) and `eventsInWindow`. An unknown name answers
  `{"error": "no such series"}`. Thinning keeps each pixel column's lowest and highest, so a spike survives.
- Analog `stats`: `count`, `failed`, `from`, `to`, `duration`, `min`, `max`, `peakToPeak`, `mean`, `rms`,
  `stddev`, `median`, `p95`, `p99`, `integral`, `timeMean`, `first`, `last`, `unit`. `mean` averages the
  samples; `timeMean` (the integral over the duration) weights by time, which is the figure to use for average
  current or power when the spacing is uneven. `integral` is the area under the trace by trapezoids and never
  spans a failed reading: charge for a series in A (coulombs, or milli and micro for `mA` and `uA`), energy
  for W. Percentiles come from an even sample once a window holds more than 262,144 values
  (`percentilesApproximate`); the rest is exact.
- Digital `stats`: `count` (edges), `from`, `to`, `duration`, `highTime`, `lowTime`, `duty`, `rising`,
  `falling`, `frequency`, `period`, `minHigh`, `maxHigh`, `minLow`, `maxLow`, `distinct` (the values a bus
  took). A pulse counts only when both edges are inside the window; the last level lasts until the newest time
  the series has seen.
- Events `stats`: `count`, `first`, `last`.

## Limits

| What | Limit |
| --- | --- |
| Series per project; plots per project | 256; 40 |
| Lanes per plot; traces per lane; traces per plot; markers | 32; 8; 64; 8 |
| Samples kept per series (`maxPoints`) | default 1,000,000, from 100 to 10,000,000; events default 20,000, up to 200,000 |
| Samples held across all projects | 30,000,000 (an event counts as 16); beyond that the biggest series lose their oldest tenth, the writer's project first |
| Samples or events in one request; items in a `batch` | 2,000,000; 256 |
| Series named in one call (plot, query, clear, delete) | 64, once each |
| Characters: series name, unit, label, group, event text, plot title | 64, 16, 160, 64, 200, 60; one line each |
| `offset`; `rate` | at most 1e12 seconds either way; above 0 and at most 1e12 |
| Followed files | 8 running per project; 256 columns or keys; 4096 VCD signals; a line of at most 1 MB; read in chunks of 8 MiB, polled every 150 ms; a missing file is waited for 60 seconds |
| Hook body | 16 MiB |

What survives a Workbench restart: series definitions (names, units, groups, offsets) and plots. Samples do
not.
