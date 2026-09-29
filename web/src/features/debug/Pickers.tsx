// "Debug…" (a launch configuration) and "Attach to Process…" pickers, in the look of
// the command palette (cmdk).

import { Command as Cmdk } from 'cmdk'
import { AlertTriangle, Cpu, Info, Plug } from 'lucide-react'
import { FEATURES, useUnsupported } from '@/api/health'
import { Kbd, Spinner } from '@/ui'
import { attachTo, startDebug } from './actions'
import { useConfigs, useProcesses } from './api'
import { ORIGIN_ICON } from './icons'
import { groupConfigs } from './logic'
import { usePicker } from './store'

function DebugPicker({ pid, close }: { pid: string; close: () => void }) {
  const q = useConfigs(pid)
  const groups = groupConfigs(q.data?.configs ?? [])
  return (
    <>
      <Cmdk.Input placeholder="Debug a launch configuration…" autoFocus />
      <Cmdk.List>
        {q.isLoading && (
          <div className="wb-dbg-picker-note">
            <Spinner /> Loading configurations…
          </div>
        )}
        <Cmdk.Empty>No matching launch configuration.</Cmdk.Empty>
        {groups.map((g) => (
          <Cmdk.Group key={g.title} heading={g.title}>
            {g.items.map((c) => {
              const I = ORIGIN_ICON[c.origin]
              return (
                <Cmdk.Item
                  key={c.name}
                  value={c.name}
                  keywords={[c.adapterLabel ?? '', c.program ?? '', c.module ?? '', c.preLaunch ?? '']}
                  onSelect={() => {
                    close()
                    void startDebug(pid, c.name)
                  }}
                >
                  <I size={15} />
                  <span className="wb-grow wb-ellipsis">{c.name}</span>
                  {c.problems.length > 0 && <AlertTriangle size={13} className="wb-warning" aria-label={c.problems.join('\n')} />}
                  <span className="wb-muted wb-small">{c.adapterLabel ?? 'no adapter'}</span>
                </Cmdk.Item>
              )
            })}
          </Cmdk.Group>
        ))}
        <Cmdk.Group heading="Other">
          <Cmdk.Item value="__attach" keywords={['attach', 'process', 'pid']} onSelect={() => usePicker.getState().set('attach')}>
            <Plug size={15} />
            <span className="wb-grow">Attach to Process…</span>
            <Kbd>Ctrl+Alt+F5</Kbd>
          </Cmdk.Item>
        </Cmdk.Group>
      </Cmdk.List>
    </>
  )
}

function AttachPicker({ pid, config, close }: { pid: string; config: string | null; close: () => void }) {
  const q = useProcesses(pid, true)
  const list = q.data?.processes ?? []
  // Where gdb cannot attach (Windows), the server picks another adapter: say so up front.
  const gdbNote = useUnsupported(FEATURES.gdbAttach)
  return (
    <>
      <Cmdk.Input placeholder={config ? `Attach “${config}” to a process: filter by name, pid or command…` : 'Attach to a process: filter by name, pid or command…'} autoFocus />
      <Cmdk.List>
        {q.data?.ptraceHint && (
          <div className="wb-dbg-picker-note warning">
            <AlertTriangle size={14} />
            <span>{q.data.ptraceHint}</span>
          </div>
        )}
        {gdbNote && (
          <div className="wb-dbg-picker-note">
            <Info size={14} />
            <span>{gdbNote}</span>
          </div>
        )}
        {q.isLoading && (
          <div className="wb-dbg-picker-note">
            <Spinner /> Listing processes…
          </div>
        )}
        {q.isError && <div className="wb-dbg-picker-note warning">{String((q.error as Error)?.message ?? q.error)}</div>}
        <Cmdk.Empty>No matching process.</Cmdk.Empty>
        {list.map((p) => (
          <Cmdk.Item
            key={p.pid}
            value={`${p.pid} ${p.name} ${p.command}`}
            onSelect={() => {
              close()
              // A launch configuration without a pid: attach it (its adapter and
              // settings) to this process.
              if (config) void startDebug(pid, config, undefined, p.pid)
              else void attachTo(pid, { pid: p.pid, language: p.language === 'python' ? 'python' : undefined })
            }}
          >
            <Cpu size={15} />
            <span className="wb-dbg-pid">{p.pid}</span>
            <span className="wb-dbg-pname">{p.name}</span>
            <span className="wb-grow wb-ellipsis wb-muted wb-small" title={p.command}>
              {p.command}
            </span>
          </Cmdk.Item>
        ))}
      </Cmdk.List>
    </>
  )
}

export function PickerHost() {
  const { kind, pid, config, set } = usePicker()
  const close = () => set(null)
  return (
    <Cmdk.Dialog
      open={!!kind && !!pid}
      onOpenChange={(o) => !o && close()}
      label={kind === 'attach' ? 'Attach to process' : 'Debug'}
      className="wb-palette wb-dbg-picker"
      overlayClassName="wb-palette-overlay"
      // Processes: every word typed must occur in the pid, name or command line
      // (cmdk's fuzzy match would find "sleep" in any long command).
      filter={kind === 'attach' ? (value, search) => (search.toLowerCase().split(/\s+/).every((w) => value.toLowerCase().includes(w)) ? 1 : 0) : undefined}
    >
      {pid && kind === 'debug' && <DebugPicker pid={pid} close={close} />}
      {pid && kind === 'attach' && <AttachPicker pid={pid} config={config} close={close} />}
    </Cmdk.Dialog>
  )
}
