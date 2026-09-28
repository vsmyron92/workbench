// The Problems tool window (bottom, Alt+6): the language servers' diagnostics of the
// current file or the whole project, grouped by file, most severe first. A click
// opens the file at the problem.

import { useMemo, useState } from 'react'
import { Braces, ChevronDown, ChevronRight, CircleCheck, CircleX, FileCode, Info, Lightbulb, RefreshCw, TriangleAlert } from 'lucide-react'
import { Button, EmptyState, ErrorBox, IconButton, Loading, Spinner } from '@/ui'
import type { LspDiagnostic } from './api'
import { useLspRuntime } from './client'
import { useLspDiagnostics, useLspStatus } from './hooks'
import { activeServers, filterDiagnostics, severityOf, type Severity } from './logic'
import { openLocation } from './nav'
import { usePopups } from './store'

const SHOW_KEY = 'wb.lsp.problems.show'

function loadShow(): Record<Severity, boolean> {
  try {
    const v = JSON.parse(localStorage.getItem(SHOW_KEY) ?? 'null')
    if (v && typeof v === 'object') return { error: v.error !== false, warning: v.warning !== false, info: v.info !== false, hint: v.hint === true }
  } catch {
    /* unavailable */
  }
  return { error: true, warning: true, info: true, hint: false }
}

export function SeverityIcon({ d, size = 13 }: { d: Pick<LspDiagnostic, 'severity'>; size?: number }) {
  switch (severityOf(d)) {
    case 'error':
      return <CircleX size={size} className="lsp-sev error" />
    case 'warning':
      return <TriangleAlert size={size} className="lsp-sev warning" />
    case 'info':
      return <Info size={size} className="lsp-sev info" />
    default:
      return <Lightbulb size={size} className="lsp-sev hint" />
  }
}

export function ProblemsWindow({ projectId }: { projectId: string | null }) {
  const st = useLspStatus(projectId)
  const enabled = !!st.data?.enabled
  const diags = useLspDiagnostics(projectId, enabled)
  const activeUri = useLspRuntime((s) => s.activeUri)
  const [tab, setTab] = useState<'file' | 'project'>('project')
  const [show, setShowState] = useState(loadShow)
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const setShow = (k: Severity) => {
    const next = { ...show, [k]: !show[k] }
    setShowState(next)
    try {
      localStorage.setItem(SHOW_KEY, JSON.stringify(next))
    } catch {
      /* quota */
    }
  }

  const files = useMemo(() => {
    const all = diags.data?.files ?? []
    const list = tab === 'file' ? all.filter((f) => f.uri === activeUri) : all
    return list.map((f) => ({ ...f, shown: filterDiagnostics(f.diagnostics, show) })).filter((f) => f.shown.length)
  }, [diags.data, tab, activeUri, show])
  const fileCount = useMemo(() => (diags.data?.files ?? []).find((f) => f.uri === activeUri)?.diagnostics.length ?? 0, [diags.data, activeUri])
  const counts = st.data?.counts

  if (!projectId) return <EmptyState icon={Braces} title="No project" />
  if (st.error) return <ErrorBox error={st.error} onRetry={() => void st.refetch()} />
  if (!st.data) return <Loading />
  if (!enabled) {
    return (
      <EmptyState
        icon={Braces}
        title="Code intelligence is off for this project"
        action={
          <Button size="small" variant="primary" onClick={() => usePopups.getState().set({ enable: { projectId } })}>
            Enable…
          </Button>
        }
      >
        Language servers report errors and warnings here once it is enabled.
      </EmptyState>
    )
  }
  const running = activeServers(st.data)

  return (
    <div className="wb-fill lsp-problems">
      <div className="lsp-problems-bar">
        <div className="lsp-tabs" role="tablist">
          <button role="tab" aria-selected={tab === 'file'} className={tab === 'file' ? 'active' : ''} onClick={() => setTab('file')}>
            Current File{fileCount ? <span className="lsp-tab-count">{fileCount}</span> : null}
          </button>
          <button role="tab" aria-selected={tab === 'project'} className={tab === 'project' ? 'active' : ''} onClick={() => setTab('project')}>
            Project{counts && counts.errors + counts.warnings + counts.infos ? <span className="lsp-tab-count">{counts.errors + counts.warnings + counts.infos}</span> : null}
          </button>
        </div>
        <span className="wb-grow" />
        <IconButton icon={CircleX} size="small" label={`Errors${counts ? ` (${counts.errors})` : ''}`} active={show.error} onClick={() => setShow('error')} />
        <IconButton icon={TriangleAlert} size="small" label={`Warnings${counts ? ` (${counts.warnings})` : ''}`} active={show.warning} onClick={() => setShow('warning')} />
        <IconButton icon={Info} size="small" label={`Information${counts ? ` (${counts.infos})` : ''}`} active={show.info} onClick={() => setShow('info')} />
        <IconButton icon={Lightbulb} size="small" label={`Hints${counts ? ` (${counts.hints})` : ''}`} active={show.hint} onClick={() => setShow('hint')} />
        <span className="lsp-bar-sep" />
        <IconButton icon={ChevronRight} size="small" label="Collapse all" onClick={() => setCollapsed(new Set(files.map((f) => f.uri)))} />
        <IconButton icon={ChevronDown} size="small" label="Expand all" onClick={() => setCollapsed(new Set())} />
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => void diags.refetch()} />
      </div>
      <div className="wb-scroll lsp-problems-list" role="tree">
        {diags.error && <ErrorBox error={diags.error} onRetry={() => void diags.refetch()} />}
        {!diags.data && diags.isFetching && <Loading />}
        {diags.data && !files.length && (
          <EmptyState icon={running.length ? CircleCheck : Braces} title={tab === 'file' && !activeUri ? 'No file is focused' : running.length ? 'No problems found' : 'No language server is running'}>
            {running.length
              ? `${running.map((s) => s.label).join(', ')}${running.some((s) => s.state === 'indexing' || s.state === 'starting') ? ' (still indexing)' : ''}`
              : 'Open a file of this project to start its language server.'}
          </EmptyState>
        )}
        {files.map((f) => {
          const open = !collapsed.has(f.uri)
          const name = f.path.slice(f.path.lastIndexOf('/') + 1)
          const dir = f.path.slice(0, Math.max(0, f.path.lastIndexOf('/')))
          return (
            <div key={f.uri} role="treeitem" aria-expanded={open}>
              <div
                className="wb-list-row lsp-file-row"
                onClick={() =>
                  setCollapsed((s) => {
                    const n = new Set(s)
                    if (open) n.add(f.uri)
                    else n.delete(f.uri)
                    return n
                  })
                }
              >
                {open ? <ChevronDown size={13} className="wb-subtle" /> : <ChevronRight size={13} className="wb-subtle" />}
                <FileCode size={13} className="wb-subtle" />
                <span className="lsp-file-name">{name}</span>
                <span className="wb-subtle wb-small wb-ellipsis">{dir}</span>
                <span className="wb-subtle wb-small">
                  {f.shown.length} problem{f.shown.length === 1 ? '' : 's'}
                </span>
              </div>
              {open &&
                f.shown.map((d, i) => (
                  <div
                    key={i}
                    className="wb-list-row lsp-diag-row"
                    title={d.message}
                    onClick={() => openLocation(f.uri, d.range)}
                  >
                    <SeverityIcon d={d} />
                    <span className="lsp-diag-msg wb-ellipsis">{d.message.split('\n')[0]}</span>
                    <span className="lsp-diag-src wb-ellipsis">
                      {[d.source ?? d.server, d.code !== undefined && d.code !== null ? String(d.code) : ''].filter(Boolean).join(' ')}
                    </span>
                    <span className="lsp-diag-pos">
                      :{d.range.start.line + 1}:{d.range.start.character + 1}
                    </span>
                  </div>
                ))}
            </div>
          )
        })}
        {diags.data?.truncated && <div className="lsp-note">Only the first 5000 problems are listed.</div>}
      </div>
      {diags.isFetching && diags.data && (
        <span className="lsp-problems-busy">
          <Spinner size={10} />
        </span>
      )}
    </div>
  )
}
