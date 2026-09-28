// Latency sparkline of an environment's recent health samples (inline SVG, tokens only).

import { formatLatency, HISTORY, sparkline } from './logic'
import type { Sample } from './types'

/** `fluid` stretches the chart to its container's width (strokes keep their width). */
export function Sparkline({ samples, width = 120, height = 20, fluid }: { samples: Sample[]; width?: number; height?: number; fluid?: boolean }) {
  const s = sparkline(samples, width, height)
  const last = samples.at(-1)
  const title = samples.length
    ? `Last ${samples.length} checks (of ${HISTORY}) · max ${formatLatency(s.max)}${last && !last.ok ? ' · last check failed' : ''}`
    : 'No checks yet'
  return (
    <svg
      className="wb-apps-spark"
      width={fluid ? '100%' : width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio={fluid ? 'none' : undefined}
      role="img"
      aria-label={title}
    >
      <title>{title}</title>
      <line className="base" x1={0} x2={width} y1={height - 0.5} y2={height - 0.5} />
      {s.line && <polyline className="line" points={s.line} />}
      {s.fails.map((x, i) => (
        <line key={i} className="fail" x1={x} x2={x} y1={1} y2={height - 1} />
      ))}
    </svg>
  )
}
