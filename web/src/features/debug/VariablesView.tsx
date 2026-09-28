// Variables of the selected frame, with watches on top (CLion's inline watches) and an
// "Evaluate expression" field. Children load lazily; big arrays load in pages;
// values that changed since the previous stop are highlighted; values can be set when
// the debugger supports it. Everything is keyed by the session's stop epoch.

import { useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Glasses, Plus, X } from 'lucide-react'
import { create } from 'zustand'
import { IconButton, Input, showMenu, Spinner } from '@/ui'
import { toast, toastError } from '@/shell/actions'
import { addWatch, setWatches } from './actions'
import { debugApi, debugKeys, inTurn, useBreakpoints } from './api'
import { useDebug } from './store'
import type { DebugSession, Scope, Variable } from './types'

const PAGE = 100

/** Which tree nodes are open, by session and path (kept across steps, like CLion). */
const useTree = create<{ open: Record<string, boolean>; set: (k: string, v: boolean) => void }>()((set) => ({
  open: {},
  set: (k, v) => set((s) => ({ open: { ...s.open, [k]: v } })),
}))

/** Values seen at the previous stop, to mark the ones that changed. */
const seen = new Map<string, { epoch: number; value: string; prev?: string }>()
function changed(key: string, epoch: number, value: string): boolean {
  const e = seen.get(key)
  if (!e) {
    if (seen.size > 20_000) seen.clear()
    seen.set(key, { epoch, value })
    return false
  }
  if (e.epoch !== epoch) {
    const next = { epoch, value, prev: e.value }
    seen.set(key, next)
    return next.prev !== value
  }
  if (e.value !== value) e.value = value
  return e.prev !== undefined && e.prev !== value
}

interface Ctx {
  s: DebugSession
  frameId: number
}

function copy(text: string) {
  void navigator.clipboard?.writeText(text).then(
    () => toast('success', 'Copied', { timeout: 1500 }),
    () => toast('error', 'Could not copy'),
  )
}

function display(v: Variable): string {
  if (v.value) return v.value
  if (v.indexedVariables) return `[${v.indexedVariables}]`
  return v.variablesReference ? '{…}' : ''
}

function VarRow({
  ctx,
  v,
  path,
  depth,
  parentRef,
  watch,
}: {
  ctx: Ctx
  v: Variable
  path: string
  depth: number
  parentRef?: number
  watch?: { expr: string; onRemove: () => void; error?: string }
}) {
  const qc = useQueryClient()
  const key = `${ctx.s.id}|${path}`
  const open = useTree((t) => t.open[key] ?? false)
  const setOpen = useTree((t) => t.set)
  const [editing, setEditing] = useState(false)
  const hasKids = v.variablesReference > 0
  const isChanged = !watch?.error && changed(key, ctx.s.stopEpoch, v.value)
  const canSet = !!parentRef && !!v.name && ctx.s.capabilities.supportsSetVariable
  const submit = async (value: string) => {
    setEditing(false)
    if (!v.name || !parentRef || value === v.value) return
    try {
      await debugApi.setVariable(ctx.s.projectId, ctx.s.id, parentRef, v.name, value)
      // Other values may depend on it: reload what this stop shows.
      void qc.invalidateQueries({ queryKey: debugKeys.session(ctx.s.id) })
    } catch (e) {
      toastError(e, `Could not set ${v.name}`)
    }
  }
  const menu = (e: React.MouseEvent) =>
    showMenu(e, [
      { label: 'Copy Value', run: () => copy(v.value) },
      { label: 'Copy Name', disabled: !v.name, run: () => copy(v.evaluateName ?? v.name ?? '') },
      { label: 'Set Value…', disabled: !canSet, run: () => setEditing(true) },
      { label: 'Add to Watches', disabled: !!watch || !(v.evaluateName ?? v.name), run: () => void addWatch(ctx.s.projectId, v.evaluateName ?? v.name ?? '') },
      ...(watch ? ([{ label: 'Remove Watch', run: watch.onRemove }] as const) : []),
    ])
  return (
    <>
      <div
        className={['wb-dbg-var', watch && 'watch', isChanged && 'changed'].filter(Boolean).join(' ')}
        style={{ paddingLeft: 6 + depth * 14 }}
        onClick={() => hasKids && setOpen(key, !open)}
        onDoubleClick={(e) => {
          if (canSet) {
            e.stopPropagation()
            setEditing(true)
          }
        }}
        onContextMenu={menu}
        title={v.type ? `${v.type}` : undefined}
      >
        <span className="twisty">{hasKids ? open ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : null}</span>
        {watch && <Glasses size={13} className="wb-dbg-watch-icon" />}
        <span className="name">{watch ? watch.expr : (v.name ?? '')}</span>
        <span className="eq">=</span>
        {editing ? (
          <Input
            small
            autoFocus
            defaultValue={v.value}
            className="wb-dbg-edit"
            onClick={(e) => e.stopPropagation()}
            onBlur={() => setEditing(false)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit((e.target as HTMLInputElement).value)
              if (e.key === 'Escape') setEditing(false)
            }}
          />
        ) : watch?.error ? (
          <span className="value error">{watch.error}</span>
        ) : (
          <span className="value">{display(v)}</span>
        )}
        {v.type && !editing && <span className="type">{v.type}</span>}
        {watch && (
          <IconButton
            className="remove"
            size="small"
            icon={X}
            label="Remove watch"
            onClick={(e) => {
              e.stopPropagation()
              watch.onRemove()
            }}
          />
        )}
      </div>
      {open && hasKids && <Children ctx={ctx} v={v} path={path} depth={depth + 1} />}
    </>
  )
}

function Children({ ctx, v, path, depth }: { ctx: Ctx; v: Variable; path: string; depth: number }) {
  const indexed = v.indexedVariables ?? 0
  // Big arrays: pages of PAGE elements (named members come first, unpaged).
  if (indexed > PAGE * 2) {
    const pages = Math.min(Math.ceil(indexed / PAGE), 200)
    return (
      <>
        {Array.from({ length: pages }, (_, i) => (
          <PageNode key={i} ctx={ctx} v={v} path={`${path}/[${i}]`} depth={depth} start={i * PAGE} count={Math.min(PAGE, indexed - i * PAGE)} />
        ))}
      </>
    )
  }
  return <VarList ctx={ctx} parent={v.variablesReference} path={path} depth={depth} />
}

function PageNode({ ctx, v, path, depth, start, count }: { ctx: Ctx; v: Variable; path: string; depth: number; start: number; count: number }) {
  const key = `${ctx.s.id}|${path}`
  const open = useTree((t) => t.open[key] ?? false)
  const setOpen = useTree((t) => t.set)
  return (
    <>
      <div className="wb-dbg-var" style={{ paddingLeft: 6 + depth * 14 }} onClick={() => setOpen(key, !open)}>
        <span className="twisty">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
        <span className="name">
          [{start} … {start + count - 1}]
        </span>
      </div>
      {open && <VarList ctx={ctx} parent={v.variablesReference} path={path} depth={depth + 1} page={{ start, count }} />}
    </>
  )
}

function VarList({ ctx, parent, path, depth, page }: { ctx: Ctx; parent: number; path: string; depth: number; page?: { start: number; count: number } }) {
  const q = useQuery({
    queryKey: [...debugKeys.session(ctx.s.id), 'vars', ctx.s.stopEpoch, parent, page?.start ?? -1],
    queryFn: ({ signal }) => debugApi.variables(ctx.s.projectId, ctx.s.id, parent, page, signal),
    staleTime: Infinity,
    retry: false,
  })
  if (q.isLoading) {
    return (
      <div className="wb-dbg-var dim" style={{ paddingLeft: 6 + depth * 14 }}>
        <Spinner size={11} /> <span className="wb-muted">loading…</span>
      </div>
    )
  }
  if (q.isError) {
    return (
      <div className="wb-dbg-var" style={{ paddingLeft: 6 + depth * 14 }}>
        <span className="value error">{(q.error as Error).message}</span>
      </div>
    )
  }
  const vars = q.data?.variables ?? []
  if (!vars.length) {
    return (
      <div className="wb-dbg-var dim" style={{ paddingLeft: 20 + depth * 14 }}>
        <span className="wb-muted">no members</span>
      </div>
    )
  }
  return (
    <>
      {vars.map((c, i) => (
        <VarRow key={`${c.name}:${i}`} ctx={ctx} v={c} path={`${path}/${c.name ?? i}`} depth={depth} parentRef={parent} />
      ))}
      {q.data?.truncated && (
        <div className="wb-dbg-var dim" style={{ paddingLeft: 20 + depth * 14 }}>
          <span className="wb-muted">… more members not shown</span>
        </div>
      )}
    </>
  )
}

function ScopeNode({ ctx, sc, first }: { ctx: Ctx; sc: Scope; first: boolean }) {
  const key = `${ctx.s.id}|scope:${sc.name}`
  // Open by default: locals and arguments (or the first scope), never registers,
  // globals or anything the debugger calls expensive.
  const local = sc.presentationHint === 'locals' || sc.presentationHint === 'arguments' || /^(locals?|arguments)$/i.test(sc.name)
  const open = useTree((t) => t.open[key] ?? (!sc.expensive && (local || (first && sc.presentationHint !== 'registers'))))
  const setOpen = useTree((t) => t.set)
  return (
    <>
      <div className="wb-dbg-var scope" onClick={() => setOpen(key, !open)}>
        <span className="twisty">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
        <span className="name">{sc.name}</span>
      </div>
      {open && <VarList ctx={ctx} parent={sc.variablesReference} path={`scope:${sc.name}`} depth={1} />}
    </>
  )
}

function Watch({ ctx, expr, index, all }: { ctx: Ctx; expr: string; index: number; all: string[] }) {
  const manual = useDebug((st) => st.manualWatches[ctx.s.id]?.includes(expr) ?? false)
  const { id: sid, projectId, stopEpoch } = ctx.s
  const q = useQuery({
    queryKey: [...debugKeys.session(sid), 'watch', stopEpoch, ctx.frameId, expr],
    // One watch at a time: a stop inside a function one of them called is then
    // blamed on that one, and the others do not evaluate into that stop.
    queryFn: () =>
      inTurn(`watch:${sid}`, async () => {
        if (useDebug.getState().sessions[sid]?.stopEpoch !== stopEpoch) throw new Error('The program moved on')
        return debugApi.evaluate(projectId, sid, expr, 'watch', ctx.frameId)
      }),
    staleTime: Infinity,
    retry: false,
    enabled: !manual,
  })
  const remove = () => void setWatches(ctx.s.projectId, all.filter((_, i) => i !== index))
  if (manual && !q.data && !q.isError && !q.isFetching) {
    return (
      <div className="wb-dbg-var watch" style={{ paddingLeft: 6 }} title="Evaluating it stopped the program (it calls a function with a breakpoint): it is evaluated only when you click">
        <span className="twisty" />
        <Glasses size={13} className="wb-dbg-watch-icon" />
        <span className="name">{expr}</span>
        <span className="eq">=</span>
        <button type="button" className="wb-dbg-link" onClick={() => void q.refetch()}>
          evaluate
        </button>
        <IconButton className="remove" size="small" icon={X} label="Remove watch" onClick={remove} />
      </div>
    )
  }
  const v: Variable = q.data ?? { name: expr, value: q.isFetching ? '…' : '', variablesReference: 0 }
  return <VarRow ctx={ctx} v={v} path={`watch:${expr}`} depth={0} watch={{ expr, onRemove: remove, error: q.isError ? (q.error as Error).message : undefined }} />
}

/** A stop inside a function a watch called: say so, and how to get out. */
function EvaluationStop({ s }: { s: DebugSession }) {
  const e = s.stopped?.duringEvaluation
  if (!e) return null
  return (
    <div className="wb-dbg-evalstop">
      Stopped inside a function called by evaluating <code>{e}</code>. Resume (F9) to let it return; this watch is now evaluated only on request.
    </div>
  )
}

function EvalBar({ s, frameId }: { s: DebugSession; frameId: number }) {
  const [text, setText] = useState('')
  const [result, setResult] = useState<{ expr: string; v?: Variable; error?: string; epoch: number } | null>(null)
  const ref = useRef<HTMLInputElement>(null)
  const evaluate = async () => {
    const expr = text.trim()
    if (!expr) return
    try {
      const v = await debugApi.evaluate(s.projectId, s.id, expr, 'watch', frameId)
      setResult({ expr, v, epoch: s.stopEpoch })
    } catch (e) {
      setResult({ expr, error: (e as Error).message, epoch: s.stopEpoch })
    }
  }
  const onKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && e.shiftKey) {
      e.preventDefault()
      if (text.trim()) void addWatch(s.projectId, text.trim())
      setText('')
    } else if (e.key === 'Enter') {
      e.preventDefault()
      void evaluate()
    } else if (e.key === 'Escape') {
      setResult(null)
    }
  }
  const ctx = { s, frameId }
  const current = result && result.epoch === s.stopEpoch ? result : null
  return (
    <>
      <div className="wb-dbg-evalbar">
        <Input
          ref={ref}
          small
          value={text}
          placeholder="Evaluate expression (Enter) or add a watch (Ctrl+Shift+Enter)"
          spellCheck={false}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKey}
          aria-label="Evaluate expression"
        />
        <IconButton
          icon={Plus}
          size="small"
          label="Add to watches (Ctrl+Shift+Enter)"
          disabled={!text.trim()}
          onClick={() => {
            void addWatch(s.projectId, text.trim())
            setText('')
          }}
        />
      </div>
      {current && (
        <div className="wb-dbg-evalresult">
          {current.error ? (
            <div className="wb-dbg-var">
              <span className="twisty" />
              <span className="name">{current.expr}</span>
              <span className="eq">=</span>
              <span className="value error">{current.error}</span>
            </div>
          ) : (
            current.v && <VarRow ctx={ctx} v={{ ...current.v, name: current.expr }} path={`eval:${current.expr}`} depth={0} />
          )}
        </div>
      )}
    </>
  )
}

export function VariablesView({ s }: { s: DebugSession }) {
  const stack = useDebug((st) => st.stacks[s.id])
  const frameIndex = useDebug((st) => st.selection[s.id]?.frameIndex ?? 0)
  const bps = useBreakpoints(s.projectId)
  const frame = stack && stack.epoch === s.stopEpoch ? stack.frames[frameIndex] : undefined
  const scopes = useQuery({
    queryKey: [...debugKeys.session(s.id), 'scopes', s.stopEpoch, frame?.id],
    queryFn: ({ signal }) => debugApi.scopes(s.projectId, s.id, frame!.id, signal),
    enabled: s.state === 'stopped' && !!frame,
    staleTime: Infinity,
    retry: false,
  })
  if (s.state !== 'stopped' || !frame) {
    return (
      <div className="wb-dbg-vars">
        <div className="wb-dbg-note wb-muted">{s.state === 'stopped' ? 'Select a frame.' : 'Variables show while the program is suspended.'}</div>
      </div>
    )
  }
  const ctx = { s, frameId: frame.id }
  const watches = bps.data?.watches ?? []
  return (
    <div className="wb-dbg-vars">
      <EvalBar s={s} frameId={frame.id} />
      <EvaluationStop s={s} />
      <div className="wb-scroll wb-dbg-tree" role="tree" aria-label="Variables">
        {watches.map((w, i) => (
          <Watch key={`${w}:${i}`} ctx={ctx} expr={w} index={i} all={watches} />
        ))}
        {scopes.isLoading && (
          <div className="wb-dbg-note">
            <Spinner /> Loading variables…
          </div>
        )}
        {scopes.isError && <div className="wb-dbg-note wb-danger">{(scopes.error as Error).message}</div>}
        {scopes.data?.scopes.map((sc, i) => <ScopeNode key={sc.name} ctx={ctx} sc={sc} first={i === 0} />)}
      </div>
    </div>
  )
}
