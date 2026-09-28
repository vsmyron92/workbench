// Breakpoints of the project (CLion's View Breakpoints, Ctrl+Shift+F8): line
// breakpoints by file with their verification in running sessions, function
// breakpoints, exception breakpoints of the debuggers used so far, mute, remove all.

import { useState } from 'react'
import { AlertTriangle, CircleSlash, Pencil, Plus, Trash2, X } from 'lucide-react'
import { Button, Checkbox, EmptyState, ErrorBox, IconButton, Input, Loading, Section, Toolbar } from '@/ui'
import { confirmDialog, toastError } from '@/shell/actions'
import { removeBreakpoint, setMuted, updateBreakpoint } from './actions'
import { applyBreakpoints, debugApi, useBreakpoints } from './api'
import { openBreakpointDialog } from './BreakpointDialog'
import { glyphKind, glyphTitle } from './logic'
import type { BreakpointsView as View, LineBreakpoint } from './types'
import { openPanel } from '@/shell/actions'

function openAt(pid: string, b: LineBreakpoint) {
  openPanel({
    kind: 'editor',
    id: `editor:${pid}:${b.path}`,
    title: b.path.split('/').pop() ?? b.path,
    params: { projectId: pid, path: b.path, line: b.line, column: 1, t: Date.now() },
  })
}

function summary(b: LineBreakpoint): string {
  const parts: string[] = []
  if (b.logMessage) parts.push(`log “${b.logMessage}”`)
  if (b.condition) parts.push(`if ${b.condition}`)
  if (b.hitCondition) parts.push(`hit ${b.hitCondition}`)
  return parts.join(' · ')
}

function LineRows({ pid, view }: { pid: string; view: View }) {
  const byFile = new Map<string, LineBreakpoint[]>()
  for (const b of [...view.breakpoints].sort((a, b) => a.path.localeCompare(b.path) || a.line - b.line)) {
    byFile.set(b.path, [...(byFile.get(b.path) ?? []), b])
  }
  if (!byFile.size) return <div className="wb-dbg-note wb-muted">Click the gutter left of a line number, or press Ctrl+F8 in the editor.</div>
  return (
    <>
      {[...byFile.entries()].map(([path, list]) => (
        <div key={path}>
          <div className="wb-dbg-bpfile wb-ellipsis" title={path}>
            {path}
          </div>
          {list.map((b) => {
            const kind = glyphKind(b, view.muted, view.live)
            const bad = view.live && b.enabled && b.status && !b.status.verified
            return (
              <div key={b.id} className="wb-list-row wb-dbg-bprow" onClick={() => openAt(pid, b)} title={glyphTitle(b, view.muted)}>
                <span onClick={(e) => e.stopPropagation()}>
                  <Checkbox checked={b.enabled} onChange={(v) => void updateBreakpoint(pid, b.path, b.line, { enabled: v })} />
                </span>
                <span className={`wb-dbg-dot ${kind}`} />
                <span className="wb-dbg-bpline">
                  {b.path.split('/').pop()}:{b.line}
                </span>
                <span className="wb-grow wb-ellipsis wb-muted wb-small">{summary(b)}</span>
                {bad && <AlertTriangle size={13} className="wb-warning" aria-label={b.status?.message ?? 'not placed'} />}
                <IconButton
                  icon={Pencil}
                  size="small"
                  label="Edit breakpoint"
                  onClick={(e) => {
                    e.stopPropagation()
                    openBreakpointDialog(pid, b.path, b.line)
                  }}
                />
                <IconButton
                  icon={X}
                  size="small"
                  label="Remove breakpoint"
                  onClick={(e) => {
                    e.stopPropagation()
                    void removeBreakpoint(pid, b.path, b.line)
                  }}
                />
              </div>
            )
          })}
        </div>
      ))}
    </>
  )
}

function FunctionRows({ pid, view }: { pid: string; view: View }) {
  const [name, setName] = useState('')
  const save = async (list: View['functionBreakpoints']) => {
    try {
      applyBreakpoints(pid, await debugApi.setFunctions(pid, list.map(({ status: _s, ...f }) => f)))
    } catch (e) {
      toastError(e)
    }
  }
  const add = () => {
    const n = name.trim()
    if (!n) return
    setName('')
    void save([...view.functionBreakpoints, { id: '', name: n, enabled: true }])
  }
  return (
    <>
      {view.functionBreakpoints.map((f, i) => (
        <div key={f.id} className="wb-list-row wb-dbg-bprow">
          <Checkbox checked={f.enabled} onChange={(v) => void save(view.functionBreakpoints.map((x, j) => (j === i ? { ...x, enabled: v } : x)))} />
          <span className={`wb-dbg-dot fn${f.enabled ? '' : ' bp-disabled'}`} />
          <span className="wb-dbg-bpline wb-ellipsis">{f.name}</span>
          <span className="wb-grow wb-ellipsis wb-muted wb-small">{f.condition ? `if ${f.condition}` : ''}</span>
          {view.live && f.enabled && f.status && !f.status.verified && <AlertTriangle size={13} className="wb-warning" aria-label={f.status.message ?? 'not placed'} />}
          <IconButton icon={X} size="small" label="Remove function breakpoint" onClick={() => void save(view.functionBreakpoints.filter((_, j) => j !== i))} />
        </div>
      ))}
      <form
        className="wb-dbg-addfn"
        onSubmit={(e) => {
          e.preventDefault()
          add()
        }}
      >
        <Input small value={name} onChange={(e) => setName(e.target.value)} placeholder="Break when entering a function, e.g. main or std::abort" spellCheck={false} aria-label="Function name" />
        <IconButton icon={Plus} size="small" label="Add function breakpoint" type="submit" disabled={!name.trim()} />
      </form>
    </>
  )
}

function ExceptionRows({ pid, view }: { pid: string; view: View }) {
  if (!view.exceptionFilters.length) return <div className="wb-dbg-note wb-muted">Exception kinds appear once a debugger that offers them has run.</div>
  return (
    <>
      {view.exceptionFilters.map((g) => (
        <div key={g.adapter} className="wb-dbg-exc">
          <div className="wb-dbg-bpfile">{g.label}</div>
          {g.filters.map((f) => {
            const on = g.enabled.includes(f.filter)
            return (
              <div key={f.filter} className="wb-list-row wb-dbg-bprow" title={f.description}>
                <Checkbox
                  checked={on}
                  onChange={async (v) => {
                    const next = v ? [...g.enabled, f.filter] : g.enabled.filter((x) => x !== f.filter)
                    try {
                      applyBreakpoints(pid, await debugApi.setExceptions(pid, g.adapter, next))
                    } catch (e) {
                      toastError(e)
                    }
                  }}
                >
                  {f.label}
                </Checkbox>
              </div>
            )
          })}
        </div>
      ))}
    </>
  )
}

export function BreakpointsView({ projectId }: { projectId: string }) {
  const q = useBreakpoints(projectId)
  if (q.isLoading) return <Loading />
  if (q.isError) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  const view = q.data!
  const count = view.breakpoints.length + view.functionBreakpoints.length
  return (
    <div className="wb-dbg-bps">
      <Toolbar>
        <Button size="small" icon={CircleSlash} variant={view.muted ? 'primary' : 'ghost'} onClick={() => void setMuted(projectId, !view.muted)} title="Sessions get no breakpoints while muted">
          {view.muted ? 'Muted' : 'Mute breakpoints'}
        </Button>
        <span className="spacer" />
        <Button
          size="small"
          icon={Trash2}
          variant="ghost"
          disabled={!count}
          onClick={async () => {
            if (!(await confirmDialog({ title: 'Remove all breakpoints?', message: `${count} breakpoint${count === 1 ? '' : 's'} of this project will be removed.`, confirmLabel: 'Remove all', danger: true }))) return
            try {
              applyBreakpoints(projectId, await debugApi.clear(projectId))
            } catch (e) {
              toastError(e)
            }
          }}
        >
          Remove all
        </Button>
      </Toolbar>
      <div className="wb-scroll wb-grow">
        {count === 0 && !view.exceptionFilters.length ? (
          <EmptyState icon={CircleSlash} title="No breakpoints">
            Click the gutter left of a line number, or press Ctrl+F8 in the editor.
          </EmptyState>
        ) : null}
        <Section title="Line breakpoints" count={view.breakpoints.length}>
          <LineRows pid={projectId} view={view} />
        </Section>
        <Section title="Function breakpoints" count={view.functionBreakpoints.length} defaultOpen={view.functionBreakpoints.length > 0}>
          <FunctionRows pid={projectId} view={view} />
        </Section>
        <Section title="Exception breakpoints" defaultOpen={view.exceptionFilters.length > 0}>
          <ExceptionRows pid={projectId} view={view} />
        </Section>
      </div>
    </div>
  )
}
