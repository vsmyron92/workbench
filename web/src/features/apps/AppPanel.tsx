// The `app` panel: a live preview of an environment or a run in an iframe, with a
// URL bar, reload, open-in-browser and a device-width toggle. Environments behind
// basic auth (or that forbid framing) load through Workbench's loopback proxy.
// Iframe history is cross-origin and cannot be driven, so there is no back/forward.

import { useEffect, useRef, useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ExternalLink, Monitor, Play, RotateCw, ShieldCheck, Smartphone, Tablet } from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { Button, EmptyState, ErrorBox, IconButton, Input, Loading, Spinner } from '@/ui'
import { openExternal, proxyUrl, startRun, useEnvs, useRuns } from './api'
import { DEVICES, isActive, isLoopbackHost, parseUrl, pathOnOrigin, runUrl, type Device } from './logic'
import { useAppsPrefs } from './store'
import type { AppPanelParams } from './types'

const DEVICE_ICON = { desktop: Monitor, tablet: Tablet, phone: Smartphone }

/** Accept "localhost:5173/x" or "example.com" in the URL bar. */
function normalize(input: string): string {
  const s = input.trim()
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) return s
  const host = s.split(/[/:]/)[0]
  return `${isLoopbackHost(host) ? 'http' : 'https'}://${s}`
}

export function AppPanel({ params, setParams, visible }: PanelProps<AppPanelParams>) {
  const { projectId, env: envName, run: runName, url } = params
  const envs = useEnvs(envName ? projectId : null)
  const runs = useRuns(runName ? projectId : null)
  const env = envName ? envs.data?.find((e) => e.name === envName) : undefined
  const run = runName ? runs.data?.find((r) => r.name === runName) : undefined
  const device = useAppsPrefs((s) => s.device)
  const setDevice = useAppsPrefs((s) => s.setDevice)
  const [draft, setDraft] = useState(url)
  const [nonce, setNonce] = useState(0)
  const [loading, setLoading] = useState(true)
  const followRun = useRef(true)

  useEffect(() => setDraft(url), [url])

  // A run's URL is only known once it is ready (Vite prints it); follow it unless the user navigated.
  const liveRunUrl = run ? runUrl(run) : null
  useEffect(() => {
    if (liveRunUrl && followRun.current && liveRunUrl !== url) setParams({ ...params, url: liveRunUrl })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liveRunUrl])
  // Reload when the run *becomes* ready (a load while it was starting hit a closed port).
  const runState = run?.state
  const prevState = useRef(runState)
  useEffect(() => {
    if (runState === 'ready' && prevState.current !== undefined && prevState.current !== 'ready') setNonce((n) => n + 1)
    prevState.current = runState
  }, [runState])

  const envPath = env ? pathOnOrigin(url, env.url) : null
  const viaProxy = !!env && env.preview.mode === 'proxy' && envPath !== null
  const proxy = useQuery({
    queryKey: ['apps', 'proxy', projectId, envName, url, nonce],
    queryFn: () => proxyUrl(projectId, envName!, envPath ?? '/'),
    enabled: viaProxy && visible,
    staleTime: Infinity,
    gcTime: 0,
    retry: false,
  })

  const parsed = parseUrl(url)
  const pageIsRemote = !isLoopbackHost(location.hostname)
  const targetIsLoopback = !!parsed && isLoopbackHost(parsed.hostname)
  const src = viaProxy ? proxy.data?.url ?? null : url
  useEffect(() => setLoading(true), [src])

  const navigate = (next: string) => {
    const u = normalize(next)
    followRun.current = false
    setParams({ ...params, url: u })
    setNonce((n) => n + 1)
  }

  let body: ReactNode
  if (!parsed || !/^https?:$/.test(parsed.protocol)) {
    body = (
      <EmptyState title="This URL cannot be previewed" action={<Button onClick={() => openExternal(url)}>Open in browser</Button>}>
        Only http(s) pages can be shown here{parsed?.protocol === 'file:' ? '; open local files from the Files tool window' : ''}.
      </EmptyState>
    )
  } else if (run && !isActive(run.state)) {
    body = (
      <EmptyState
        icon={Play}
        title={`${run.name} is not running`}
        action={
          <Button variant="primary" icon={Play} onClick={() => void startRun(projectId, run.name)}>
            Run {run.name}
          </Button>
        }
      >
        {run.error ?? 'Start it to see the preview.'}
      </EmptyState>
    )
  } else if (run && run.state !== 'ready' && run.config.ready) {
    body = <Loading label={run.phase ?? `Waiting for ${run.name} to be ready…`} />
  } else if (viaProxy && proxy.isLoading) {
    body = <Loading label="Starting the preview proxy…" />
  } else if (viaProxy && proxy.error) {
    body = <ErrorBox error={proxy.error} onRetry={() => proxy.refetch()} />
  } else if (viaProxy && !proxy.data?.url) {
    body = (
      <EmptyState title="Preview not available on this device" action={<Button icon={ExternalLink} onClick={() => openExternal(url)}>Open in a new window</Button>}>
        {proxy.data?.reason}
      </EmptyState>
    )
  } else if (src) {
    const width = DEVICES[device].width
    body = (
      <div className={`wb-apps-stage ${device}`}>
        <div className="wb-apps-frame" style={width ? { width } : undefined}>
          {loading && <div className="wb-apps-loadbar" />}
          <iframe
            key={`${src}#${nonce}`}
            src={src}
            title={env ? `Preview of ${env.name}` : run ? `Preview of ${run.name}` : 'Preview'}
            onLoad={() => setLoading(false)}
            sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads"
            // No clipboard-read: the framed page can be any site (agents open URLs here), and
            // the delegation would let it read the clipboard under Workbench's permission.
            allow="clipboard-write; fullscreen"
          />
        </div>
      </div>
    )
  }

  return (
    <div className="wb-fill wb-apps-panel">
      <div className="wb-toolbar wb-apps-urlbar">
        <IconButton size="small" icon={RotateCw} label="Reload" onClick={() => setNonce((n) => n + 1)} />
        <form
          className="wb-grow wb-row"
          onSubmit={(e) => {
            e.preventDefault()
            navigate(draft)
          }}
        >
          {viaProxy && (
            <span className="wb-apps-proxy-badge" title={`Loaded through a local proxy: ${env?.preview.reason ?? ''}`}>
              <ShieldCheck size={12} /> proxy
            </span>
          )}
          <Input small className="mono wb-grow" value={draft} onChange={(e) => setDraft(e.target.value)} aria-label="URL" spellCheck={false} />
        </form>
        {loading && src && <Spinner size={12} />}
        <div className="wb-apps-devices" role="radiogroup" aria-label="Device width">
          {(Object.keys(DEVICES) as Device[]).map((d) => (
            <IconButton key={d} size="small" icon={DEVICE_ICON[d]} label={DEVICES[d].label} active={device === d} onClick={() => setDevice(d)} />
          ))}
        </div>
        <IconButton size="small" icon={ExternalLink} label="Open in browser" onClick={() => openExternal(url)} />
      </div>
      {pageIsRemote && targetIsLoopback && (
        <div className="wb-apps-banner wb-small">
          This preview points at <code>{parsed?.host}</code> on the Workbench machine; it only loads when this device can reach it.
        </div>
      )}
      <div className="wb-apps-body">{body}</div>
    </div>
  )
}
