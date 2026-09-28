// Status bar: the project's language servers at a glance (state, indexing progress)
// and its problem counts. A click opens the popover: enable/disable, where servers
// run, each server with its command, state, Restart / Stop / Log / Off here, and
// install hints for missing ones.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Ban, Braces, ChevronDown, ChevronRight, CircleX, Info, Play, Power, RotateCw, ScrollText, Square, TriangleAlert } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import { Badge, Button, ErrorBox, IconButton, Select, Spinner, StatusDot } from '@/ui'
import { useProjects } from '@/api/queries'
import { lspApi, type LspStatus, type Mode, type ServerStatus } from './api'
import { lsp } from './client'
import { useLspStatus } from './hooks'
import { progressText, serversInUse, STATE_LABEL, stateTone, summarize } from './logic'
import { usePopups } from './store'

const usePopover = (() => {
  let setter: ((v: DOMRect | null) => void) | null = null
  return {
    open: (r: DOMRect | null) => setter?.(r),
    bind: (s: (v: DOMRect | null) => void) => {
      setter = s
      return () => {
        if (setter === s) setter = null
      }
    },
  }
})()

/** Open the popover (palette command). */
export function openLspPopover() {
  const el = document.querySelector('[data-lsp-status]')
  usePopover.open(el?.getBoundingClientRect() ?? new DOMRect(8, window.innerHeight - 24, 10, 20))
}

export function LspStatusWidget({ projectId }: { projectId: string | null }) {
  const st = useLspStatus(projectId)
  const [anchor, setAnchor] = useState<DOMRect | null>(null)
  useEffect(() => usePopover.bind(setAnchor), [])
  const s = st.data
  if (!projectId || !s) return null
  const sum = summarize(s)
  const anyRelevant = s.servers.some((x) => x.relevant && x.enabled)
  if (!s.enabled && !anyRelevant && !anchor) return null
  const c = s.counts
  return (
    <>
      <button
        className={`wb-status-item lsp-status ${sum.tone}`}
        data-lsp-status
        title={s.enabled ? `Code intelligence: ${sum.text}` : 'Code intelligence is off for this project'}
        onClick={(e) => setAnchor(anchor ? null : e.currentTarget.getBoundingClientRect())}
      >
        <Braces size={12} />
        {sum.busy ? <Spinner size={9} /> : s.enabled && sum.tone !== 'muted' ? <StatusDot tone={sum.tone} /> : null}
        <span className="wb-ellipsis lsp-status-text">{sum.text}</span>
      </button>
      {s.enabled && (c.errors > 0 || c.warnings > 0) && (
        <button className="wb-status-item" title={`${c.errors} errors, ${c.warnings} warnings in ${c.files} files: show Problems`} onClick={() => showToolWindow('problems', 'bottom')}>
          {c.errors > 0 && (
            <span className="lsp-count error">
              <CircleX size={12} /> {c.errors}
            </span>
          )}
          {c.warnings > 0 && (
            <span className="lsp-count warning">
              <TriangleAlert size={12} /> {c.warnings}
            </span>
          )}
        </button>
      )}
      {anchor && createPortal(<Popover projectId={projectId} status={s} anchor={anchor} onClose={() => setAnchor(null)} />, document.body)}
    </>
  )
}

function Popover({ projectId, status, anchor, onClose }: { projectId: string; status: LspStatus; anchor: DOMRect; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  const { data: projects } = useProjects()
  const project = projects?.find((p) => p.id === projectId)
  const [showAll, setShowAll] = useState(false)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<unknown>(null)
  const [pos, setPos] = useState<{ left: number; bottom: number }>({ left: anchor.left, bottom: window.innerHeight - anchor.top + 4 })
  useLayoutEffect(() => {
    const w = ref.current?.offsetWidth ?? 460
    setPos({ left: Math.max(8, Math.min(anchor.left, window.innerWidth - w - 8)), bottom: window.innerHeight - anchor.top + 4 })
  }, [anchor])
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as HTMLElement
      if (ref.current?.contains(t) || t.closest?.('[data-lsp-status]') || t.closest?.('.wb-modal-backdrop') || t.closest?.('.wb-menu')) return
      onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !document.querySelector('.wb-modal-backdrop')) onClose()
    }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  const run = async (key: string, fn: () => Promise<LspStatus>) => {
    setBusy(key)
    setError(null)
    try {
      lsp.statusChanged(await fn())
    } catch (e) {
      setError(e)
    } finally {
      setBusy(null)
    }
  }
  const inUse = serversInUse(status)
  const others = status.servers.filter((s) => !inUse.includes(s))
  const container = status.devcontainer && status.devcontainer.state !== 'none'

  return (
    <div ref={ref} className="lsp-popover" style={pos} role="dialog" aria-label="Code intelligence">
      <div className="lsp-pop-head">
        <Braces size={15} />
        <div className="wb-grow">
          <div className="lsp-pop-title">Code Intelligence</div>
          <div className="wb-small wb-muted wb-ellipsis">{project?.name ?? projectId}</div>
        </div>
        {status.enabled ? (
          <Button size="small" icon={Power} loading={busy === 'disable'} onClick={() => void run('disable', () => lspApi.disable(projectId))}>
            Disable
          </Button>
        ) : (
          <Button
            size="small"
            variant="primary"
            icon={Power}
            onClick={() => {
              onClose()
              usePopups.getState().set({ enable: { projectId } })
            }}
          >
            Enable…
          </Button>
        )}
      </div>
      {!status.enabled && (
        <div className="lsp-pop-note">
          <Info size={13} />
          <span>Off for this project: no language server runs for it. Enabling runs the servers below, which execute project code.</span>
        </div>
      )}
      {status.enabled && container && (
        <label className="lsp-pop-mode">
          <span className="wb-small wb-muted">Run servers</span>
          <Select value={status.mode} onChange={(e) => void run('mode', () => lspApi.settings(projectId, { mode: e.target.value as Mode }))}>
            <option value="auto">Dev container when running</option>
            <option value="container">Dev container only</option>
            <option value="host">This computer</option>
          </Select>
        </label>
      )}
      {!!error && (
        <div style={{ padding: '0 10px 8px' }}>
          <ErrorBox error={error} />
        </div>
      )}
      <div className="lsp-pop-list">
        {inUse.length === 0 && <div className="lsp-pop-empty">No language server matches this project&apos;s files yet.</div>}
        {inUse.map((s) => (
          <ServerRow key={s.id} projectId={projectId} s={s} status={status} busy={busy} run={run} onLogs={() => usePopups.getState().set({ logs: { projectId, serverId: s.id } })} />
        ))}
        {others.length > 0 && (
          <button className="lsp-pop-more" onClick={() => setShowAll(!showAll)}>
            {showAll ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            Other servers ({others.length})
          </button>
        )}
        {showAll &&
          others.map((s) => (
            <ServerRow key={s.id} compact projectId={projectId} s={s} status={status} busy={busy} run={run} onLogs={() => usePopups.getState().set({ logs: { projectId, serverId: s.id } })} />
          ))}
      </div>
      {status.warnings.length > 0 && (
        <div className="lsp-pop-note warning">
          <TriangleAlert size={13} />
          <span>{status.warnings.join('; ')}</span>
        </div>
      )}
    </div>
  )
}

function ServerRow({
  projectId,
  s,
  status,
  busy,
  run,
  onLogs,
  compact,
}: {
  projectId: string
  s: ServerStatus
  status: LspStatus
  busy: string | null
  run: (key: string, fn: () => Promise<LspStatus>) => Promise<void>
  onLogs: () => void
  compact?: boolean
}) {
  const running = ['starting', 'indexing', 'ready'].includes(s.state)
  const tone = stateTone(s.state)
  const toggleHere = () => {
    const current = status.servers.filter((x) => x.disabledHere).map((x) => x.id)
    const next = s.disabledHere ? current.filter((x) => x !== s.id) : [...current, s.id]
    void run(`here:${s.id}`, () => lspApi.settings(projectId, { disabledServers: next }))
  }
  return (
    <div className={`lsp-server${compact ? ' compact' : ''}`}>
      <div className="lsp-server-main">
        {s.state === 'starting' || s.state === 'indexing' ? <Spinner size={10} /> : <StatusDot tone={tone} />}
        <span className="lsp-server-name">{s.label}</span>
        <span className={`lsp-server-state ${tone}`}>{running ? progressText(s) : STATE_LABEL[s.state]}</span>
        {s.side === 'container' && <Badge tone="accent">container</Badge>}
        <span className="wb-grow" />
        {status.enabled && s.available && !s.disabledHere && s.enabled && (
          <>
            {running ? (
              <>
                <IconButton icon={RotateCw} size="small" label="Restart" disabled={busy !== null} onClick={() => void run(`restart:${s.id}`, () => lspApi.restart(projectId, s.id))} />
                <IconButton icon={Square} size="small" label="Stop" disabled={busy !== null} onClick={() => void run(`stop:${s.id}`, () => lspApi.stop(projectId, s.id))} />
              </>
            ) : (
              <IconButton icon={Play} size="small" label="Start" disabled={busy !== null} onClick={() => void run(`restart:${s.id}`, () => lspApi.restart(projectId, s.id))} />
            )}
          </>
        )}
        {(s.pid || s.state === 'crashed' || s.state === 'failed' || s.state === 'stopped') && <IconButton icon={ScrollText} size="small" label="Show log" onClick={onLogs} />}
        {s.enabled && (
          <IconButton icon={Ban} size="small" label={s.disabledHere ? 'Use in this project' : 'Turn off for this project'} active={s.disabledHere} disabled={busy !== null} onClick={toggleHere} />
        )}
      </div>
      {s.progress?.percentage !== undefined && s.progress?.percentage !== null && running && (
        <div className="lsp-progress">
          <div style={{ width: `${s.progress.percentage}%` }} />
        </div>
      )}
      {!compact && (
        <div className="lsp-server-sub wb-ellipsis" title={s.running ?? s.command}>
          {s.languages.join(', ')} · <code>{s.running ?? s.command}</code>
          {s.pid ? ` · pid ${s.pid}` : ''}
        </div>
      )}
      {s.error && (s.state === 'crashed' || s.state === 'failed') && <div className="lsp-server-error">{s.error}</div>}
      {!s.available && s.enabled && (
        <div className="lsp-server-hint">
          <span className="wb-muted">{s.missing}</span>
          {s.installHint && (
            <code className="lsp-code" title="Install command">
              {s.installHint}
            </code>
          )}
        </div>
      )}
    </div>
  )
}

