// Side-by-side 3D comparison (a lazily loaded chunk with three.js).
// Adapted from Mr. Mak Workspace (MIT), src/components/Compare3D.tsx: one list of
// tests, a pane per model, one shading toolbar for all panes, cameras locked
// together. Keys work while the viewer has focus: 1–8 modes, ←/→ tests, S sync,
// R re-frame, Space spin, I reference image.

import { useCallback, useEffect, useRef, useState, type MutableRefObject } from 'react'
import { ChevronLeft, ChevronRight, Eye, EyeOff, ImageIcon, Link2, Link2Off, RotateCw, Scan } from 'lucide-react'
import { ErrorBox, IconButton, Loading, Select, Spinner } from '@/ui'
import { manifestMode } from '../logic'
import { ModelViewer, type CameraState, type ShadingMode, type ViewerStats } from './viewer'

export interface Compare3DModel {
  file: string
  label: string
  logo?: string
  note?: string
  accent?: string
  rotationY?: number
  /** Cover name shown while Redact is on. */
  alias?: string
}

export interface Compare3DTest {
  id: string
  name: string
  kind?: 'lowpoly' | 'highpoly'
  defaultMode?: ShadingMode
  note?: string
  input?: string
  models: Compare3DModel[]
}

export interface Compare3DManifest {
  title?: string
  tests: Compare3DTest[]
}

const MODES: { id: ShadingMode; label: string; hint: string }[] = [
  { id: 'wire', label: 'Wireframe', hint: 'The mesh: quads where the export keeps them' },
  { id: 'solid', label: 'Clay', hint: 'Untextured: shape and silhouette only' },
  { id: 'normals', label: 'Normals', hint: 'Geometry normals: smoothing and flipped faces' },
  { id: 'pbr', label: 'Textured', hint: 'Authored materials with environment lighting' },
  { id: 'albedo', label: 'Base color', hint: 'Base colour map, unlit' },
  { id: 'normalMap', label: 'Normal map', hint: 'The normal map itself; flat blue means none was baked' },
  { id: 'rough', label: 'Roughness', hint: 'Roughness map, or the flat material value' },
  { id: 'metal', label: 'Metalness', hint: 'Metalness map, or the flat material value' },
]

const fmt = (n: number) => n.toLocaleString('en-US')

function texLabel(px: number): string {
  if (!px) return 'untextured'
  return px % 1024 === 0 ? `${px / 1024}K tex` : `${px}px tex`
}

/** Validate an untrusted manifest just enough to render it. */
export function parseManifest(v: unknown): Compare3DManifest {
  const m = v as Compare3DManifest
  if (!m || typeof m !== 'object' || !Array.isArray(m.tests) || !m.tests.length) throw new Error('The manifest has no tests')
  const tests = m.tests
    .filter((t) => t && Array.isArray(t.models))
    .map((t, i) => ({
      ...t,
      id: String(t.id ?? i),
      name: String(t.name ?? `Test ${i + 1}`),
      defaultMode: manifestMode(t.defaultMode),
      models: t.models.filter((x) => x && typeof x.file === 'string').map((x) => ({ ...x, label: String(x.label ?? x.file) })),
    }))
    .filter((t) => t.models.length)
  if (!tests.length) throw new Error('No test in the manifest lists a model file')
  return { title: typeof m.title === 'string' ? m.title : undefined, tests }
}

function Pane({
  url,
  model,
  redacted,
  onToggleReveal,
  logoUrl,
  mode,
  syncRef,
  viewers,
}: {
  url: string
  model: Compare3DModel
  redacted: boolean
  onToggleReveal: () => void
  logoUrl?: string
  mode: ShadingMode
  syncRef: MutableRefObject<((from: ModelViewer, s: CameraState) => void) | null>
  viewers: MutableRefObject<Set<ModelViewer>>
}) {
  const hostRef = useRef<HTMLDivElement>(null)
  const viewerRef = useRef<ModelViewer | null>(null)
  const modeRef = useRef(mode)
  modeRef.current = mode
  const [stats, setStats] = useState<ViewerStats | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [err, setErr] = useState('')

  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    let cancelled = false
    let viewer: ModelViewer
    try {
      viewer = new ModelViewer(host)
    } catch (e) {
      setErr(e instanceof Error ? e.message : 'WebGL is not available')
      setStatus('error')
      return
    }
    viewerRef.current = viewer
    viewers.current.add(viewer)
    viewer.onStats = (s) => !cancelled && setStats(s)
    viewer.onCameraChange = (s) => syncRef.current?.(viewer, s)
    viewer.setMode(modeRef.current)
    setStatus('loading')
    setStats(null)
    viewer
      .load(url, model.rotationY)
      .then(() => !cancelled && setStatus('ready'))
      .catch((e: unknown) => {
        if (cancelled) return
        setErr(e instanceof Error ? e.message : String(e))
        setStatus('error')
      })
    return () => {
      cancelled = true
      viewers.current.delete(viewer)
      viewerRef.current = null
      viewer.dispose()
    }
  }, [url, model.rotationY, syncRef, viewers])

  useEffect(() => {
    viewerRef.current?.setMode(mode)
  }, [mode])

  const name = model.alias && redacted ? model.alias : model.label
  return (
    <div className="c3d-pane">
      <div className="c3d-pane-head">
        {logoUrl && <img className="c3d-pane-logo" src={logoUrl} alt="" />}
        <span className={`c3d-pane-name${model.alias && redacted ? ' redacted' : ''}`} style={model.accent && !(model.alias && redacted) ? { color: model.accent } : undefined}>
          {name}
        </span>
        {model.alias && <IconButton icon={redacted ? Eye : EyeOff} size="small" label={redacted ? 'Reveal the model name' : 'Hide the model name'} onClick={onToggleReveal} />}
        {model.note && <span className="c3d-pane-sub">{model.note}</span>}
        {stats && (
          <span className="c3d-pane-stats">
            {fmt(stats.triangles)} tris · {fmt(stats.vertices)} verts · {stats.meshes} {stats.meshes === 1 ? 'mesh' : 'meshes'} · {texLabel(stats.textureSize)}
          </span>
        )}
      </div>
      <div className="c3d-stage">
        <div className="c3d-canvas" ref={hostRef} />
        {status === 'loading' && (
          <div className="ws-overlay">
            <Spinner size={18} />
            <span>Loading model…</span>
          </div>
        )}
        {status === 'error' && (
          <div className="ws-overlay error">
            <strong>Could not load {model.file}</strong>
            <span className="wb-small">{err}</span>
          </div>
        )}
      </div>
    </div>
  )
}

export default function Compare3D({ manifestUrl, manifest: given }: { manifestUrl: string; manifest?: Compare3DManifest }) {
  const [manifest, setManifest] = useState<Compare3DManifest | null>(given ?? null)
  const [error, setError] = useState<string | null>(null)
  const [testIndex, setTestIndex] = useState(0)
  // Mode is remembered per test: a low-poly test opens on its wireframe, and
  // coming back lands where you left it.
  const [modeByTest, setModeByTest] = useState<Record<string, ShadingMode>>({})
  const [sync, setSync] = useState(true)
  const [spin, setSpin] = useState(false)
  const [redact, setRedact] = useState<boolean | null>(null)
  const [showInput, setShowInput] = useState(false)
  const viewers = useRef<Set<ModelViewer>>(new Set())
  const syncRef = useRef<((from: ModelViewer, s: CameraState) => void) | null>(null)
  const syncOn = useRef(sync)
  syncOn.current = sync

  useEffect(() => {
    if (given) {
      setManifest(given)
      return
    }
    const ctl = new AbortController()
    setManifest(null)
    setError(null)
    fetch(manifestUrl, { signal: ctl.signal, credentials: 'omit' })
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((m: unknown) => setManifest(parseManifest(m)))
      .catch((e: unknown) => !ctl.signal.aborted && setError(e instanceof Error ? e.message : String(e)))
    return () => ctl.abort()
  }, [manifestUrl, given])

  useEffect(() => {
    syncRef.current = (from, state) => {
      if (!syncOn.current) return
      for (const v of viewers.current) if (v !== from) v.applyCamera(state)
    }
    return () => {
      syncRef.current = null
    }
  }, [])

  const tests = manifest?.tests ?? []
  const count = tests.length
  const test = tests[Math.min(testIndex, Math.max(0, count - 1))]
  const mode: ShadingMode = test ? (modeByTest[test.id] ?? test.defaultMode ?? (test.kind === 'lowpoly' ? 'wire' : 'pbr')) : 'solid'
  const hasAlias = tests.some((t) => t.models.some((m) => m.alias))
  const redacted = redact ?? hasAlias

  useEffect(() => {
    for (const v of viewers.current) v.setAutoRotate(spin)
  }, [spin, testIndex, mode])

  // A narrow bar scrolls the modes: keep the active one visible.
  const segRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const seg = segRef.current
    const b = seg?.querySelector<HTMLElement>('button.active')
    if (!seg || !b) return
    if (b.offsetLeft < seg.scrollLeft) seg.scrollLeft = b.offsetLeft
    else if (b.offsetLeft + b.offsetWidth > seg.scrollLeft + seg.clientWidth) seg.scrollLeft = b.offsetLeft + b.offsetWidth - seg.clientWidth
  }, [mode, manifest])

  const setMode = useCallback((m: ShadingMode) => test && setModeByTest((prev) => ({ ...prev, [test.id]: m })), [test])
  const reframe = useCallback(() => {
    for (const v of viewers.current) v.frame()
  }, [])
  const step = (d: number) => count && setTestIndex((i) => (i + d + count) % count)
  const resolve = (rel: string) => new URL(rel, new URL(manifestUrl, location.href)).pathname

  if (error) return <ErrorBox error={new Error(`Could not read the comparison manifest: ${error}`)} />
  if (!manifest || !test) return <Loading label="Loading comparison…" />

  return (
    <div
      className="c3d"
      tabIndex={0}
      onKeyDown={(ev) => {
        const t = ev.target as HTMLElement
        if (t.tagName === 'SELECT' || t.tagName === 'INPUT') return
        const n = Number(ev.key)
        if (n >= 1 && n <= MODES.length) setMode(MODES[n - 1].id)
        else if (ev.key === 'ArrowLeft') step(-1)
        else if (ev.key === 'ArrowRight') step(1)
        else if (ev.key === 'i' || ev.key === 'I') setShowInput((v) => !v)
        else if (ev.key === 's' || ev.key === 'S') setSync((v) => !v)
        else if (ev.key === 'r' || ev.key === 'R') reframe()
        else if (ev.code === 'Space') setSpin((v) => !v)
        else return
        ev.preventDefault()
      }}
    >
      <div className="c3d-bar">
        {count > 1 && (
          <div className="wb-row" style={{ gap: 2 }}>
            <IconButton icon={ChevronLeft} size="small" label="Previous test (←)" onClick={() => step(-1)} />
            <Select value={String(testIndex)} onChange={(e) => setTestIndex(Number(e.target.value))} aria-label="Test" title={test.note ?? test.name}>
              {tests.map((t, i) => (
                <option key={t.id} value={i}>
                  {t.name}
                </option>
              ))}
            </Select>
            <IconButton icon={ChevronRight} size="small" label="Next test (→)" onClick={() => step(1)} />
            <span className="wb-small wb-muted">
              {testIndex + 1}/{count}
            </span>
          </div>
        )}
        {count === 1 && <span className="c3d-title wb-ellipsis">{manifest.title ?? test.name}</span>}
        <div className="ws-seg small" role="group" aria-label="Shading mode" ref={segRef}>
          {MODES.map((m, i) => (
            <button key={m.id} className={mode === m.id ? 'active' : ''} onClick={() => setMode(m.id)} title={`${m.hint} · ${i + 1}`}>
              {m.label}
            </button>
          ))}
        </div>
        <span className="spacer" />
        <div className="wb-row" style={{ gap: 2 }}>
          {test.models.length > 1 && <IconButton icon={sync ? Link2 : Link2Off} size="small" label="Lock cameras together (S)" active={sync} onClick={() => setSync(!sync)} />}
          <IconButton icon={RotateCw} size="small" label="Spin (Space)" active={spin} onClick={() => setSpin(!spin)} />
          <IconButton icon={Scan} size="small" label="Re-frame (R)" onClick={reframe} />
          {test.input && <IconButton icon={ImageIcon} size="small" label="Reference image (I)" active={showInput} onClick={() => setShowInput(!showInput)} />}
        </div>
      </div>
      {test.note && count > 1 && <div className="c3d-note wb-small wb-muted">{test.note}</div>}
      <div className="c3d-panes" style={{ gridTemplateColumns: `repeat(${Math.min(test.models.length, 3)}, minmax(0, 1fr))` }}>
        {test.models.map((m) => (
          <Pane
            key={`${test.id}:${m.file}`}
            url={resolve(m.file)}
            model={m}
            redacted={redacted}
            onToggleReveal={() => setRedact(!redacted)}
            logoUrl={m.logo ? resolve(m.logo) : undefined}
            mode={mode}
            syncRef={syncRef}
            viewers={viewers}
          />
        ))}
      </div>
      {test.input && showInput && (
        <div className="c3d-input" onClick={() => setShowInput(false)} role="presentation">
          <img src={resolve(test.input)} alt={`Reference image for ${test.name}`} />
          <span className="wb-small">Reference image: click or press I to close</span>
        </div>
      )}
    </div>
  )
}
