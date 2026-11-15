/* Workbench report charts: Chart.js (../_shared/chart.umd.js, load it first) in the report's dark look.
   Sets Chart's defaults from the report tokens (report.css), gives series the palette below when a dataset
   names no colours, and defines window.wbPalette and window.wbAlpha(colour, alpha). Wrap each canvas in
   <div class="wb-chart"> (relative, 280 px high; set its height to change it). No network, no dependencies. */
(() => {
  const css = getComputedStyle(document.documentElement);
  const token = (name, fallback) => css.getPropertyValue(name).trim() || fallback;
  const hex = /^#[0-9a-f]{6}$/i;
  // Series colours in order: the report's accent, good, warn and bad, then four more that stay apart on dark.
  const palette = [
    ['--wbr-accent', '#548af7'],
    ['--wbr-good', '#5fb865'],
    ['--wbr-warn', '#e0b050'],
    ['--wbr-bad', '#e55765'],
    ['--wbr-c5', '#4fb3b0'],
    ['--wbr-c6', '#a87be0'],
    ['--wbr-c7', '#e08a50'],
    ['--wbr-c8', '#d96aa7'],
  ].map(([name, fallback]) => {
    const value = token(name, fallback);
    return hex.test(value) ? value : fallback;
  });
  window.wbPalette = palette;
  /** `wbAlpha('#548af7', 0.2)` → '#548af733' (a six-digit hex colour with an alpha). */
  window.wbAlpha = (colour, alpha) => (hex.test(colour) ? colour + Math.round(Math.max(0, Math.min(1, alpha)) * 255).toString(16).padStart(2, '0') : colour);

  const style = document.createElement('style');
  style.textContent = '.wb-chart{position:relative;height:280px;min-width:0}.wb-chart canvas{max-width:100%}';
  document.head.append(style);

  const Chart = window.Chart;
  if (!Chart) return;
  const text = token('--wbr-text', '#dfe1e5');
  const muted = token('--wbr-muted', '#9da0a8');
  const border = token('--wbr-border', '#393b40');
  const surface = token('--wbr-surface', '#2b2d30');
  const D = Chart.defaults;
  D.color = muted;
  D.borderColor = border;
  D.font.family = token('--wbr-font', "system-ui, -apple-system, 'Segoe UI', sans-serif");
  D.font.size = 12;
  D.responsive = true;
  D.maintainAspectRatio = false;
  D.animation = matchMedia('(prefers-reduced-motion: reduce)').matches ? false : { duration: 250 };
  D.plugins.legend.labels.color = text;
  D.plugins.legend.labels.usePointStyle = true;
  D.plugins.title.color = text;
  D.plugins.tooltip.backgroundColor = surface;
  D.plugins.tooltip.borderColor = border;
  D.plugins.tooltip.borderWidth = 1;
  D.plugins.tooltip.titleColor = text;
  D.plugins.tooltip.bodyColor = text;
  D.plugins.colors.enabled = false;
  D.elements.line.tension = 0.2;
  D.elements.point.radius = 2;
  D.elements.bar.borderRadius = 3;

  // A dataset that names no colour gets the next one of the palette (a pie or doughnut: one per slice).
  Chart.register({
    id: 'wbPalette',
    beforeUpdate(chart) {
      const type = chart.config.type;
      const sliced = type === 'pie' || type === 'doughnut' || type === 'polarArea';
      const filled = type === 'bar' || sliced;
      chart.config.data.datasets.forEach((dataset, i) => {
        const colour = palette[i % palette.length];
        if (sliced) {
          if (dataset.backgroundColor === undefined) dataset.backgroundColor = dataset.data.map((_, j) => palette[j % palette.length]);
          if (dataset.borderColor === undefined) dataset.borderColor = surface;
        } else {
          if (dataset.borderColor === undefined) dataset.borderColor = colour;
          if (dataset.backgroundColor === undefined) dataset.backgroundColor = filled ? colour : window.wbAlpha(colour, 0.2);
          if (!filled && dataset.pointBackgroundColor === undefined) dataset.pointBackgroundColor = colour;
        }
      });
    },
  });
})();
