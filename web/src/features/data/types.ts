// Shapes of the data slice's REST and events (server/src/data). Times are seconds.

export type Kind = 'analog' | 'digital' | 'events'

export interface SeriesInfo {
  name: string
  kind: Kind
  unit: string
  label: string
  group: string
  offset: number
  maxPoints: number
  rate: number | null
  /** Samples held (events: events). */
  count: number
  /** Samples taken since the last clear, rolled-off ones included. */
  appended: number
  dropped: number
  tStart: number | null
  tEnd: number | null
  /** Times are Unix seconds: a clock rather than time since an instrument started. */
  wall: boolean
  /** The newest value (not for events). */
  last: number | null
  /** Bumped by every clear: a view held for an older one is stale. */
  generation: number
  updatedAt: number
}

export type TraceStyle = '' | 'line' | 'step' | 'dots'

export interface Trace {
  series: string
  /** 1–8: the chart colour (`--plot-N`); 0: none (digital and events are drawn in the text colour). */
  color: number
  hidden?: boolean
  style: TraceStyle
}

export interface Lane {
  title: string
  traces: Trace[]
  min: number | null
  max: number | null
  log?: boolean
  /** Height relative to the other lanes; 0: by kind. */
  weight: number
}

export interface PlotConfig {
  id: string
  title: string
  lanes: Lane[]
  /** Seconds in view while following; 0: everything captured. */
  span: number
  follow: boolean
  linked: boolean
  markers: string[]
}

export interface SourceInfo {
  id: string
  path: string
  format: 'csv' | 'jsonl' | 'vcd'
  follow: boolean
  state: 'waiting' | 'running' | 'done' | 'stopped' | 'error'
  rows: number
  samples: number
  skipped: number
  bytes: number
  error: string | null
  series: string[]
  startedAt: number
  updatedAt: number
}

export interface Snapshot {
  series: SeriesInfo[]
  plots: PlotConfig[]
  sources: SourceInfo[]
}

/** What the server returns for one series over a window (`POST …/data/view`). */
export interface SeriesView {
  kind?: Kind
  missing?: boolean
  generation?: number
  tStart?: number | null
  tEnd?: number | null
  count?: number
  /** Samples in the window before thinning. */
  n?: number
  thinned?: boolean
  t?: number[]
  /** null: a failed reading. */
  v?: (number | null)[]
  events?: [number, string][]
}

export interface RangeStats {
  kind?: Kind
  unit?: string
  missing?: boolean
  count?: number
  failed?: number
  from?: number | null
  to?: number | null
  duration?: number
  min?: number
  max?: number
  peakToPeak?: number
  mean?: number
  rms?: number
  stddev?: number
  median?: number
  p95?: number
  p99?: number
  integral?: number
  timeMean?: number | null
  first?: number
  last?: number
  // digital
  highTime?: number
  lowTime?: number
  duty?: number | null
  rising?: number
  falling?: number
  frequency?: number | null
  period?: number | null
  minHigh?: number | null
  maxHigh?: number | null
  minLow?: number | null
  maxLow?: number | null
  distinct?: number
}

export interface PlotParams {
  projectId: string
  plotId: string
}
