// The chip's registers by name (the configuration's `svd`, the vendor's CMSIS-SVD file):
// peripherals, their registers with values read from the halted target, and each register's
// bit fields with the vendor's value names. A register or a field is changed with a
// double-click. Values are read when the program is suspended, once per stop (like the
// variables); registers whose read changes the chip (a status flag that clears when read)
// are left alone until their eye button is pressed.

import { useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Eye, RefreshCw } from 'lucide-react'
import { create } from 'zustand'
import { EmptyState, ErrorBox, IconButton, Input, Loading, showMenuAt, Spinner } from '@/ui'
import { toastError } from '@/shell/actions'
import { debugApi, debugKeys } from './api'
import { fieldRange, fieldValueText, filterPeripherals, filterRegisters, hexAddress, registerNote } from './logic'
import type { DebugSession, SvdField, SvdPeripheralSummary, SvdRegister } from './types'

/** Which peripherals and registers are open, by session. */
const useOpen = create<{ open: Record<string, boolean>; set: (k: string, v: boolean) => void }>()((set) => ({
  open: {},
  set: (k, v) => set((s) => ({ open: { ...s.open, [k]: v } })),
}))

/** Values seen at the previous stop, to mark the registers that changed. */
const seen = new Map<string, { epoch: number; value: string; prev?: string }>()
function changed(key: string, epoch: number, value: string | null | undefined): boolean {
  if (!value) return false
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
  e.value = value
  return e.prev !== undefined && e.prev !== value
}

function FieldRow({ s, peripheral, r, f, depth }: { s: DebugSession; peripheral: string; r: SvdRegister; f: SvdField; depth: number }) {
  const qc = useQueryClient()
  const [editing, setEditing] = useState(false)
  const anchor = useRef<HTMLDivElement>(null)
  // A field is written by reading its register, changing the bits and writing it back: not for a register that reading changes.
  const canEdit = s.state === 'stopped' && r.value != null && !r.readAction && r.access !== 'read-only' && f.access !== 'read-only'
  const write = async (value: string | number) => {
    setEditing(false)
    try {
      await debugApi.svdWrite(s.projectId, s.id, peripheral, r.name, { field: f.name, value })
      void qc.invalidateQueries({ queryKey: [...debugKeys.session(s.id), 'svd', peripheral] })
    } catch (e) {
      toastError(e, `Could not change ${peripheral}.${r.name}.${f.name}`)
    }
  }
  const edit = () => {
    if (!canEdit) return
    if (f.values.length && anchor.current) {
      showMenuAt(anchor.current, [
        ...f.values.map((v) => ({ label: `${v.name} (${v.value})`, run: () => void write(v.value) })),
        'separator' as const,
        { label: 'Number…', run: () => setEditing(true) },
      ])
    } else {
      setEditing(true)
    }
  }
  return (
    <div ref={anchor} className="wb-dbg-var wb-dbg-field" style={{ paddingLeft: 6 + depth * 14 }} onDoubleClick={edit} title={f.description ?? undefined}>
      <span className="twisty" />
      <span className="addr">{fieldRange(f)}</span>
      <span className="name">{f.name}</span>
      {f.value != null && <span className="eq">=</span>}
      {editing ? (
        <Input
          small
          autoFocus
          defaultValue={String(f.value ?? 0)}
          className="wb-dbg-edit"
          onBlur={() => setEditing(false)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void write((e.target as HTMLInputElement).value)
            if (e.key === 'Escape') setEditing(false)
          }}
        />
      ) : (
        <span className="value">{fieldValueText(f)}</span>
      )}
    </div>
  )
}

function RegisterRow({ s, peripheral, r, force }: { s: DebugSession; peripheral: string; r: SvdRegister; force: (name: string) => void }) {
  const qc = useQueryClient()
  const key = `${s.id}|${peripheral}|${r.name}`
  const open = useOpen((o) => o.open[key] ?? false)
  const setOpen = useOpen((o) => o.set)
  const [editing, setEditing] = useState(false)
  const isChanged = changed(key, s.stopEpoch, r.value)
  const canEdit = s.state === 'stopped' && r.access !== 'read-only'
  const note = registerNote(r)
  const write = async (value: string) => {
    setEditing(false)
    if (!value.trim() || value.trim() === r.value) return
    try {
      await debugApi.svdWrite(s.projectId, s.id, peripheral, r.name, { value: value.trim() })
      void qc.invalidateQueries({ queryKey: [...debugKeys.session(s.id), 'svd', peripheral] })
    } catch (e) {
      toastError(e, `Could not write ${peripheral}.${r.name}`)
    }
  }
  return (
    <>
      <div
        className={['wb-dbg-var wb-dbg-reg', isChanged && 'changed'].filter(Boolean).join(' ')}
        style={{ paddingLeft: 20 }}
        onClick={() => r.fields.length && setOpen(key, !open)}
        onDoubleClick={(e) => {
          if (canEdit) {
            e.stopPropagation()
            setEditing(true)
          }
        }}
        title={[r.description, `${hexAddress(r.address)} · ${r.size} bits · ${r.access}${r.resetValue ? ` · reset ${r.resetValue}` : ''}`].filter(Boolean).join('\n')}
      >
        <span className="twisty">{r.fields.length ? open ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : null}</span>
        <span className="name">{r.name}</span>
        <span className="addr">{hexAddress(r.address)}</span>
        {r.value != null && <span className="eq">=</span>}
        {editing ? (
          <Input
            small
            autoFocus
            defaultValue={r.value ?? '0x'}
            className="wb-dbg-edit"
            onClick={(e) => e.stopPropagation()}
            onBlur={() => setEditing(false)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void write((e.target as HTMLInputElement).value)
              if (e.key === 'Escape') setEditing(false)
            }}
          />
        ) : (
          r.value != null && <span className="value hex">{r.value}</span>
        )}
        {note && <span className={`note${r.error ? ' error' : ''}`}>{note}</span>}
        {r.skipped && r.access !== 'write-only' && s.state === 'stopped' && (
          <IconButton
            size="small"
            icon={Eye}
            label={`Read ${r.name} (${r.skipped})`}
            onClick={(e) => {
              e.stopPropagation()
              force(r.name)
            }}
          />
        )}
      </div>
      {open && r.fields.map((f) => <FieldRow key={f.name} s={s} peripheral={peripheral} r={r} f={f} depth={2} />)}
    </>
  )
}

function Registers({ s, name, filter }: { s: DebugSession; name: string; filter: string }) {
  const [forced, setForced] = useState<string[]>([])
  const stopped = s.state === 'stopped'
  const q = useQuery({
    queryKey: [...debugKeys.session(s.id), 'svd', name, s.stopEpoch, stopped, forced.join(',')],
    queryFn: ({ signal }) => debugApi.svdPeripheral(s.projectId, s.id, name, stopped, forced, signal),
    staleTime: Infinity,
    retry: false,
    // The previous values stay while the new ones load (no flicker at every step).
    placeholderData: (prev) => prev,
  })
  if (q.isLoading) {
    return (
      <div className="wb-dbg-var dim" style={{ paddingLeft: 20 }}>
        <Spinner size={11} /> <span className="wb-muted">reading…</span>
      </div>
    )
  }
  if (q.isError) {
    return (
      <div className="wb-dbg-var" style={{ paddingLeft: 20 }}>
        <span className="value error">{(q.error as Error).message}</span>
      </div>
    )
  }
  const regs = filterRegisters(q.data?.registers ?? [], filter)
  if (!regs.length) {
    return (
      <div className="wb-dbg-var dim" style={{ paddingLeft: 20 }}>
        <span className="wb-muted">no matching registers</span>
      </div>
    )
  }
  return (
    <>
      {regs.map((r) => (
        <RegisterRow key={r.name} s={s} peripheral={name} r={r} force={(n) => setForced((f) => (f.includes(n) ? f : [...f, n]))} />
      ))}
    </>
  )
}

function PeripheralNode({ s, p, registerFilter }: { s: DebugSession; p: SvdPeripheralSummary; registerFilter: string }) {
  const key = `${s.id}|${p.name}`
  const open = useOpen((o) => o.open[key] ?? false)
  const setOpen = useOpen((o) => o.set)
  return (
    <>
      <div className="wb-dbg-var scope wb-dbg-peri-node" onClick={() => setOpen(key, !open)} title={p.description ?? undefined}>
        <span className="twisty">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
        <span className="name">{p.name}</span>
        <span className="addr">{hexAddress(p.base)}</span>
        <span className="note wb-ellipsis">{p.description ?? ''}</span>
      </div>
      {open && <Registers s={s} name={p.name} filter={registerFilter} />}
    </>
  )
}

export function PeripheralsView({ s }: { s: DebugSession }) {
  const [filter, setFilter] = useState('')
  const qc = useQueryClient()
  const list = useQuery({
    queryKey: ['debug', 'svd', s.id],
    queryFn: ({ signal }) => debugApi.svd(s.projectId, s.id, signal),
    staleTime: Infinity,
    retry: false,
  })
  if (list.isLoading) return <Loading label="Reading the register map…" />
  if (list.isError) return <ErrorBox error={list.error} onRetry={() => void list.refetch()} />
  const all = list.data?.peripherals ?? []
  if (!all.length) return <EmptyState title="No peripherals">The SVD file describes none.</EmptyState>
  // "uart ctrl": peripherals that match all words, else (a register query) the registers inside.
  const matches = filterPeripherals(all, filter)
  const shown = matches.length ? matches : all
  const registerFilter = matches.length ? '' : filter
  return (
    <div className="wb-dbg-peri">
      <div className="wb-dbg-peri-head">
        <Input small placeholder={`Filter ${all.length} peripherals${list.data?.device ? ` of ${list.data.device}` : ''}`} value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter peripherals" />
        <IconButton icon={RefreshCw} size="small" label="Read the open peripherals again" disabled={s.state !== 'stopped'} onClick={() => void qc.invalidateQueries({ queryKey: [...debugKeys.session(s.id), 'svd'] })} />
        <span className="wb-muted wb-small">{s.state === 'stopped' ? '' : 'Pause the program to read registers'}</span>
      </div>
      <div className="wb-scroll wb-dbg-peri-list">
        {shown.map((p) => (
          <PeripheralNode key={p.name} s={s} p={p} registerFilter={registerFilter} />
        ))}
      </div>
    </div>
  )
}
