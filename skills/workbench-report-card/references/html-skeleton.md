# HTML skeleton for a card's report

Copy the block into the card folder as `report.html` and replace the content. It uses only classes that `../_shared/report.css` defines, relative paths and no network, so it renders in the report sandbox. Home may also hold a card called "Hand work to an agent" with a fuller `template.html`.

```html
<!doctype html>
<html lang="en" data-wb-report="document">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Search latency after caching</title>
<link rel="stylesheet" href="../_shared/report.css">
<script defer src="../_shared/report.js"></script>
</head>
<body>
<main>
<h1>Search latency after caching</h1>
<p class="meta">Prepared by an agent for branch <code>feature/search-cache</code>. Load test of 10 minutes at 600 requests per second.</p>

<!-- Three to five numbers that answer the question. -->
<div class="kpis">
  <div class="kpi">p95 <code>/search</code><b class="good">140 ms</b></div>
  <div class="kpi">Change<b class="good">-67%</b></div>
  <div class="kpi">Cache hit rate<b>81%</b></div>
  <div class="kpi">Errors<b class="bad">3</b></div>
</div>

<!-- The answer first. -->
<div class="card">
  <h3>Summary</h3>
  <p>Caching cut the p95 of <code>/search</code> from 420 ms to 140 ms. Three requests failed while the cache warmed up and need a look before this ships.</p>
</div>

<h2>Measurements</h2>
<div class="grid-2">
  <figure>
    <img src="img/latency.png" alt="p95 latency by endpoint, before and after caching" loading="lazy">
    <figcaption>p95 per endpoint, before and after.</figcaption>
  </figure>
  <figure>
    <img src="img/traffic.png" alt="Requests per second over a day, peaking at about 650" loading="lazy">
    <figcaption>The load profile the test replayed.</figcaption>
  </figure>
</div>

<h3>By endpoint</h3>
<table>
  <thead><tr><th>Endpoint</th><th>p95 before</th><th>p95 after</th><th>Change</th></tr></thead>
  <tbody>
    <tr><td><code>/search</code></td><td>420 ms</td><td>140 ms</td><td class="good">-67%</td></tr>
    <tr><td><code>/cart</code></td><td>260 ms</td><td>250 ms</td><td class="warn">-4%</td></tr>
  </tbody>
</table>

<h2>What to do next</h2>
<div class="grid-3">
  <div class="card"><h3 class="good">Ship</h3><p>The cache for <code>/search</code>, behind the existing flag.</p></div>
  <div class="card"><h3 class="warn">Decide</h3><p>Whether <code>/cart</code> is worth caching.</p></div>
  <div class="card"><h3 class="bad">Fix first</h3><p>The three warm-up errors: a missing timeout on the first fill.</p></div>
</div>

<h2>How it was measured</h2>
<pre>k6 run --vus 200 --duration 10m load/search.js</pre>

<!-- End with what was checked and what was not. -->
<p class="meta">Checked: latency, errors and hit rate on a local build. Not checked: memory growth over hours.</p>
</main>
</body>
</html>
```

## Parts and what they are for

- `data-wb-report="document"` on `<html>` switches the styles on. Without it the page styles in `report.css` do not apply. It also gives `main` a centred column of at most 1120 px.
- The two `../_shared/` lines are right for a file directly in the card folder. A report in a subfolder needs one more `../` per level.
- `meta` is a muted line for who, from what and when (`subtitle` is the same). `kpis` holds `kpi` boxes; put the number in `<b>`. `good`, `warn` and `bad` only colour text, so use them on `<b>`, `<td>`, `<h3>` or a span.
- `card` is a bordered panel. `grid-2` and `grid-3` are equal columns that collapse to one below 680 px; `grid-4` and `gallery` are auto-filling grids with tiles at least 220 px wide, good for thumbnails.
- Every `<img>` that is not inside a link opens in the lightbox on click, with arrow keys, a 100% toggle and a download button. Its `alt` is the caption, so write one that says what the picture shows. `loading="lazy"` keeps a long report quick.
- `table` scrolls sideways by itself when it is wider than the page, as does `pre`.

## Extras

Anything beyond those classes goes into a small `<style>` in the head. Use the variables `report.css` defines: `--wbr-bg`, `--wbr-surface`, `--wbr-inset`, `--wbr-border`, `--wbr-text`, `--wbr-muted`, `--wbr-accent`, `--wbr-good`, `--wbr-warn`, `--wbr-bad`, `--wbr-font` and `--wbr-mono`.

```html
<style>
  .note { border-left: 3px solid var(--wbr-accent); padding: 4px 14px; margin: 14px 0; color: var(--wbr-muted); }
  .bars { display: block; width: 100%; max-width: 360px; }
  .bars rect { fill: var(--wbr-accent); }
  .bars text { fill: var(--wbr-muted); font-size: 9px; }
</style>

<p class="note">Numbers are medians of three runs.</p>

<!-- A chart without a library: inline SVG. -->
<svg class="bars" viewBox="0 0 240 100" role="img" aria-label="p95 in milliseconds: search 140, cart 250">
  <rect x="30" y="38" width="60" height="50"></rect>
  <rect x="140" y="4" width="60" height="84"></rect>
  <text x="38" y="98">search</text>
  <text x="152" y="98">cart</text>
</svg>
```

For data that is easier to keep in a file, write `data.json` into the card folder and read it with `fetch('data.json')` in an inline script; only files of the card folder can be fetched. Guard anything that touches `localStorage`, `sessionStorage` or cookies with `try`/`catch`: in the sandbox they throw.
