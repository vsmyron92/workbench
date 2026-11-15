# Charts and live data in a report

Needs Workbench 0.20 or later. Both are for HTML reports in a card; both work inside the report sandbox (no network, files of the card folder and `../_shared/` only).

## Charts

Chart.js 4 is part of every scope's `_shared/` folder, so no CDN and nothing to save into the card:

```html
<link rel="stylesheet" href="../_shared/report.css">
<script src="../_shared/chart.umd.js"></script>
<script src="../_shared/report-charts.js"></script>   <!-- the report's dark look and a palette; load it after chart.umd.js -->
<script defer src="../_shared/report.js"></script>

<div class="card"><div class="wb-chart"><canvas id="lat" role="img" aria-label="p95 latency by endpoint, before and after"></canvas></div></div>
<script>
  new Chart(document.getElementById('lat'), {
    type: 'bar',
    data: {
      labels: ['/search', '/cart', '/checkout'],
      datasets: [{ label: 'before', data: [420, 260, 310] }, { label: 'after', data: [140, 250, 300] }],   // no colours: the palette is used
    },
    options: { scales: { y: { title: { display: true, text: 'p95 (ms)' } } } },
  });
</script>
```

- Each `<canvas>` goes in `<div class="wb-chart">`: a relative box 280 px high (set `style="height:360px"` to change it). Charts are responsive and fill it.
- A dataset that names no colour gets the next colour of `wbPalette` (accent, good, warn, bad, then four more); a pie or doughnut gets one per slice. `wbAlpha('#548af7', 0.2)` makes a translucent version of a palette colour. Name colours yourself when they mean something (red for failures: `wbPalette[3]`).
- Give every canvas an `aria-label` that says what it shows: it is the chart's text for screen readers.
- Chart.js's plugins that need other files (zoom, annotation, date adapters) are not included: use numbers or ISO strings on a `category` or `linear` axis. Do not save another copy of Chart.js into the card.
- The card's thumbnail is taken from the first local image: a canvas is not one, so put a PNG or an inline SVG first if the card should have a picture.

## Live data

In a card of a project, `workbench.query(kind, params)` returns a promise of what Workbench knows about that project now. The user is asked once per card to **Allow** it (a strip above the report); until they answer, the promise waits. Read-only, one project (the card's), and never in Home, the Sandbox or a card from a repository.

| kind | params | answer |
|---|---|---|
| `sessions` | none | `{sessions: [{id, title, provider, state, unread, model, effort, contextPct, costUsd, lastEventAt, running}]}`; `state` is `starting`, `idle`, `working`, `needs_permission`, `needs_input`, `error` or `exited` |
| `git.repos` | none | `{repos: [{id, name, default, branch, head, upstream, ahead, behind, changed, conflicts, state, github, gitlab, error}]}`; `github` and `gitlab` are `{host, path}` or `null` |
| `ci.runs` | `limit` (1 to 30, default 10), `branch`, `provider` (`github` or `gitlab`), `repo` | `{provider, runs: [{id, title, status, conclusion, branch, sha, event, createdAt, durationSec, url}]}`, newest first. GitHub: `status` `completed` and `conclusion` `success` or `failure`. GitLab: `status` `success`, `failed`, `running`… and `conclusion: null`. `unavailable` when the project has no GitHub or GitLab remote |
| `data.series` | none | `{series: [{name, kind, unit, label, group, count, tStart, tEnd, last, wall, updatedAt}]}`: what the data viewer holds |
| `data.view` | `series` (1 to 16 names), `from`, `to` (seconds), `buckets` (1 to 2000, default 400) | `{views: {<series>: {kind, count, tStart, tEnd, t: [...], v: [...], n, thinned}}}`; `v` has `null` for a failed reading; an events series has `events: [[t, text]]`; an unknown name is `{missing: true}` |
| `data.stats` | `series`, `from`, `to` | `{stats: {<series>: {...}}}` |

A rejection is an `Error` with a `code`: `denied` (the user said no), `unavailable` (this card cannot have live data, or no forge), `busy` (too many requests: at most 20 a second), `too_large` (about 1.5 MB: ask for fewer series or buckets), `bad_request`, `failed` (Workbench's own message is in `message`) and `timeout` (two minutes without an answer). Outside Workbench (the report opened in a browser tab of its own) `workbench.query` rejects with `unavailable`.

```html
<div class="kpis" id="kpis"></div>
<script>
  async function refresh() {
    try {
      const { sessions } = await workbench.query('sessions');
      const waiting = sessions.filter((s) => s.state === 'needs_permission' || s.state === 'needs_input').length;
      const box = document.createElement('div');
      box.className = 'kpi';
      box.append('Waiting for you');
      const n = document.createElement('b');
      n.className = waiting ? 'warn' : 'good';
      n.textContent = waiting;
      box.append(n);
      document.getElementById('kpis').replaceChildren(box);
    } catch (e) {
      // denied or unavailable: say so, and show whatever the report can without it
      document.getElementById('kpis').textContent = e.code === 'denied' ? 'Live data is off for this report.' : e.message;
    }
  }
  refresh();
  setInterval(() => document.visibilityState === 'visible' && refresh(), 15000);   // no faster than every 5 seconds
</script>
```

- Always catch the rejection and draw something useful: sample data or a note, never a blank page. The Sandbox card "Sandbox playground" has a `dashboard.html` step that does this; copy it into your project's card.
- Update a chart in place (`chart.data = …; chart.update()`) instead of building a new one on every poll.
- Write text from the answer with `textContent`, not `innerHTML`: titles of sessions and runs are data, not markup.
- It is a way to read, not to act: there is no way to start a session, change a file or retry a job from a report.
- Say in the card's description that the report reads live data, so the user is not surprised by the question.
