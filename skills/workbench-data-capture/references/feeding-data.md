# Feeding the viewer: the hook and followed files

Read this before you write a script that posts samples, or before you follow a file.

## The hook: a script posts with the session's own token

A Workbench agent session has `WORKBENCH_URL` (the local server), `WORKBENCH_TERMINAL_ID` and
`WORKBENCH_AGENT_TOKEN` in its environment, and a script you run from your shell tool normally inherits them.
The token belongs to this session's terminal: it is accepted only by the hook routes and `/mcp`, and it writes
only into this session's project. Check that the variables are set without printing the token:

```bash
test -n "$WORKBENCH_URL" && test -n "$WORKBENCH_TERMINAL_ID" && test -n "$WORKBENCH_AGENT_TOKEN" && echo hook available
```

If one is empty, there is no hook for you: use `data_append` or `data_follow`.

The request is `POST $WORKBENCH_URL/api/hooks/data/$WORKBENCH_TERMINAL_ID/append` with
`Authorization: Bearer <token>` and `Content-Type: application/json`. The body is a `data_append` body without
`projectId`, or `{"batch": [ .. ]}` with up to 256 of them; at most 16 MiB. Success answers
`{"appended": [{"series", "taken", "dropped", "count", "tEnd", "created"}]}`. A failure answers
`{"error": {"code", "message"}}`: 400 says what to change and nothing was changed, 401 is a wrong token, 404 a
terminal without a project. Read the answer: `curl -s` exits 0 on a 400 (`--fail-with-body`, curl 7.76 or
newer, gives a non-zero exit and still prints the message).

Never type the token, echo it, write it into a file or a script, or put it in argv. In shell, hand curl its
header on stdin; `printf` is a shell builtin, so the token appears in no command line (`-H @-` reads header
lines from stdin; curl 7.55 or newer):

```bash
printf 'Authorization: Bearer %s\n' "$WORKBENCH_AGENT_TOKEN" |
  curl -sS -X POST "$WORKBENCH_URL/api/hooks/data/$WORKBENCH_TERMINAL_ID/append" \
    -H @- -H 'Content-Type: application/json' \
    --data-binary '{"series":"VDD","unit":"V","kind":"analog","values":[3.30,3.29,3.31],"t0":0,"dt":0.001}'
```

From a capture script in Python the token never reaches a command line at all:

```python
import json, os, urllib.request

URL = f"{os.environ['WORKBENCH_URL']}/api/hooks/data/{os.environ['WORKBENCH_TERMINAL_ID']}/append"

def post(body):
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST", headers={
        "Authorization": "Bearer " + os.environ["WORKBENCH_AGENT_TOKEN"],
        "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.load(r)
```

A 4xx raises `urllib.error.HTTPError`; print `e.read()` to see Workbench's message. A shell without network
access to the local server (a sandboxed session) cannot post: write a file and `data_follow` it.

Rules for a posting loop:

- Post batches, not single values. The viewer redraws at most every 100 ms, so five to ten posts a second show
  everything; each post is `{"series": "I", "t": [..], "v": [..]}` or
  `{"series": "I", "values": [..], "t0": 0, "rate": 10000}` for an evenly sampled stream. After the first post
  `values` without `t0` continue where the series left off (`rate` or `dt` must be known).
- Send several series in one request with `batch`. A refused item stops the batch there; the items before it
  stay and the error names the failing one.
- Post one series from one place, in time order. A post whose first time is earlier than the newest held time
  is refused.
- Take times from the instrument's clock (counted from the capture's start) or all from the wall clock, never
  both for series that share a plot.
- End a digital series with a sample at the capture's end: a repeated level is dropped, but the line is
  extended to the newest time.
- Print progress from the script (flush it), and stop the script when the capture is done.

## Following a file with `data_follow`

Start the capture tool writing into a normal folder of the project (for example `captures/run7.csv`), then
follow the file; before or after the tool starts both work, but a file that does not appear within 60 seconds
ends the source with an error. Whole lines reach the plot when the writer flushes them: make the tool or
script flush per line, or the plot fills in jumps. Keep the file: it is the record, since samples are not
saved.

| Format | Taken from the extension | What it reads |
| --- | --- | --- |
| `csv` | `.csv` `.tsv` `.dat` `.txt` `.log` | delimited or white-space separated tables, with or without a header |
| `jsonl` | `.jsonl` `.ndjson` `.json` | one JSON object per line |
| `vcd` | `.vcd` | logic analyzer and simulator waveforms |

Any other extension needs `format` (`csv`, `jsonl` or `vcd`).

**CSV.** The delimiter is the commonest of `,` `;` tab `|` in the first line (else white space; `delimiter`
overrides, `"tab"` for a tab). Blank lines and lines starting `#` or `//` are skipped. The first line is a
header when it holds text (`header` overrides); without one the columns are `col1`, `col2` and so on.
`Name [unit]` or `Name (unit)` gives a column its unit; `unit` gives one to every column that has none.

- Time: a column whose header's first word (up to `[`, `(`, `_` or a space) is `time`, `t`, `timestamp`, `ts`,
  `elapsed`, `seconds`, `sec`, `epoch`, `datetime` or `date`, or the one named by `timeColumn` (header name,
  or its 1-based number as a string). Its unit is `timeUnit` (`s`, `ms`, `us`, `ns`), else the header's
  (`Time [ms]`, `time_ms`), else seconds. ISO 8601 and `YYYY-MM-DD HH:MM:SS` values are dates (UTC when no
  zone is given) and become Unix seconds.
- No time column: `rate` puts row n at n/rate; without it, a followed file is stamped when Workbench reads the
  rows and a finished import (`follow: false`) counts rows as seconds.
- Cells: numbers (also `0x1F`); empty cells are skipped; `nan`, `na`, `n/a`, `-`, `--`, `null` are failed
  readings; `true`, `high`, `h` and `false`, `low`, `l` are levels (digital); other text becomes an event in a
  series named after the column. A numeric column with an occasional word (an instrument's `OVLD`) keeps its
  numbers and the words go to a series `<column> (text)`.
- Series are named after the columns, with `prefix` in front; repeated names get `_2`, `_3`. At most 256
  columns. `columns` takes only some (names or 1-based numbers), `digital` lists logic columns, `kind` applies
  to every numeric column.
- A series that already exists keeps its definition, so `data_series` with `action: "define"` before the
  follow gives a CSV without units the unit, kind and group you want (use the exact series name, `prefix`
  included). New series are grouped under the file's path unless you pass `group`.

**JSON lines.** `{"t": 0.5, "i": 1.5, "on": true, "state": "RX"}`: the time key is found as for CSV
(`timeColumn` names another; `timeUnit` applies), every other key is a series, booleans are digital, strings
are events, `null` is a failed reading. `{"series": "x", "t": 2, "v": 7}` carries one named sample. A line
that is not a JSON object is ignored without an error, so a file holding one JSON array reads as nothing.

**VCD.** Scalar signals become digital series named by their scope (`top.clk`), vectors up to 53 bits become
numbers (a bus), unknown bits (`x`, `z`) a gap, `real` variables analog. Times come from `$timescale` (1 ns
when it is missing). `columns` picks signals by dotted name. At most 4096 signals.

**Refused, or ends the source.**

- A hidden path component (`.cache/`), key-like names (such as `id_rsa*`, `id_ed25519*`, `.pem`, `.key`,
  `.p12`, `.pfx`, `.token`, `_token`, `_api_key`, `.keystore`, `.jks`), files the project marks sensitive, a
  path outside the project, a link that leads out or to such a file (checked again at every look, so a link
  created later is refused too). Move the capture to a normal folder such as `captures/`; do not make a link
  to get around it.
- A binary file, a line over 1 MB, a path that is not a file, an unknown extension without `format`.
- The same file twice (409), a ninth running source (stop one first).
- A file that shrinks, is replaced, or starts differently starts the capture over and clears the series that
  source wrote; a file that disappears while followed is waited for again.

**Reading a source.** `data_follow` with `action: "list"` returns for each source `id`, `path`, `state`
(`waiting`, `running`, `done`, `stopped`, `error`), `rows`, `samples`, `skipped` (samples left out because
their time went backwards or the series refused them), `bytes`, `series` and `error`. `error` may be set while
the state is still `running`: it is the last refusal. Nothing arrives while the state is `waiting`.
`action: "stop"` with the `id` ends a source; what it read stays in the series.
