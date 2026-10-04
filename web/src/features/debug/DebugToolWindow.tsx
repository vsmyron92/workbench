// The Debug tool window (bottom, CLion's layout): session tabs and the launch
// configuration picker on top, the session toolbar (rerun, resume/pause, stop, step
// over/into/out, run to cursor, breakpoints, mute, ask agent), then Threads &
// Variables, Console and Breakpoints.

import { useMemo, useRef, type ReactNode } from 'react'
import {
  ArrowDownToDot,
  ArrowUpFromDot,
  BugPlay,
  ChevronDown,
  CircleDot,
  CircleSlash,
  Cpu,
  Pause,
  Play,
  Plug,
  RedoDot,
  RotateCw,
  Sparkles,
  Square,
  SquareTerminal,
  TextCursorInput,
  X,
} from 'lucide-react'
import { EmptyState, IconButton, showMenuAt, Spinner, Splitter, StatusDot, Tabs, type MenuEntry } from '@/ui'
import { openPanel } from '@/shell/actions'
import { askAgentAboutStop, control, forget, rerun, runToCursor, setMuted, startDebug, stopSession, viewBreakpoints } from './actions'
import { useBreakpoints, useConfigs } from './api'
import { BreakpointsView } from './BreakpointsView'
import { ConsoleView } from './ConsoleView'
import { lastFocusedEditor } from './editor'
import { FramesView } from './FramesView'
import { LiveWatchView } from './LiveWatchView'
import { PeripheralsView } from './PeripheralsView'
import { configIcon } from './icons'
import { configTitle, defaultConfig, groupConfigs, isLive, remoteChip, stateLabel, stateTone } from './logic'
import { StartView } from './StartView'
import { activeSession, openAttachPicker, sessionsOf, useDebug, useDebugPrefs, type DebugTab } from './store'
import type { DebugSession } from './types'
import { VariablesView } from './VariablesView'

function ConfigPicker({ projectId, compact }: { projectId: string; compact?: boolean }) {
  const q = useConfigs(projectId)
  const picked = useDebugPrefs((s) => s.config[projectId])
  const list = q.data?.configs ?? []
  const current = list.find((c) => c.name === picked) ?? defaultConfig(list, q.data?.lastConfig)
  const open = (el: HTMLElement) => {
    const items: MenuEntry[] = []
    for (const g of groupConfigs(list)) {
      if (items.length) items.push('separator')
      for (const c of g.items.slice(0, 40)) {
        items.push({ label: c.problems.length ? `${c.name}  ⚠` : c.name, icon: configIcon(c), run: () => useDebugPrefs.getState().setConfig(projectId, c.name) })
      }
    }
    if (items.length) items.push('separator')
    items.push({ label: 'Attach to Process…', icon: Plug, run: () => openAttachPicker(projectId) })
    showMenuAt(el, items)
  }
  return (
    <div className="wb-dbg-launch" role="group" aria-label="Launch configuration">
      <button className="wb-dbg-launch-sel" onClick={(e) => open(e.currentTarget)} disabled={q.isLoading} title={current ? configTitle(current) : 'No launch configurations'}>
        {q.isLoading ? <Spinner size={11} /> : null}
        <span className="wb-ellipsis">{current?.name ?? 'No configurations'}</span>
        <ChevronDown size={13} className="wb-muted" />
      </button>
      <IconButton icon={BugPlay} label={`Debug ${current?.name ?? ''} (Shift+F9)`} className="wb-dbg-go" disabled={!current} onClick={() => current && void startDebug(projectId, current.name)} />
      {!compact && <IconButton icon={Plug} label="Attach to Process… (Ctrl+Alt+F5)" onClick={() => openAttachPicker(projectId)} />}
    </div>
  )
}

function SessionTabs({ list, active, projectId }: { list: DebugSession[]; active: DebugSession | null; projectId: string }) {
  const setActive = useDebug((s) => s.setActive)
  return (
    <div className="wb-dbg-stabs" role="tablist" aria-label="Debug sessions">
      {list.map((s) => (
        <div
          key={s.id}
          role="tab"
          aria-selected={s.id === active?.id}
          className={`wb-dbg-stab${s.id === active?.id ? ' active' : ''}`}
          onClick={() => setActive(projectId, s.id)}
          title={`${s.name} — ${stateLabel(s)}${s.error ? `\n${s.error}` : ''}`}
        >
          <StatusDot tone={stateTone(s)} pulse={s.state === 'starting'} />
          <span className="wb-ellipsis">{s.name}</span>
          {!isLive(s) && (
            <IconButton
              icon={X}
              size="small"
              label="Close"
              className="close"
              onClick={(e) => {
                e.stopPropagation()
                void forget(s)
              }}
            />
          )}
        </div>
      ))}
    </div>
  )
}

function SessionToolbar({ s }: { s: DebugSession }) {
  const bps = useBreakpoints(s.projectId)
  const live = isLive(s)
  const stopped = s.state === 'stopped'
  const runTo = () => {
    const e = lastFocusedEditor()
    const pos = e?.editor.getPosition()
    if (e?.projectId && pos) void runToCursor(e.projectId, e.path, pos.lineNumber)
  }
  return (
    <div className="wb-dbg-toolbar" role="toolbar" aria-label="Debug session">
      <IconButton icon={RotateCw} label="Rerun" disabled={!s.config} onClick={() => void rerun(s)} />
      {s.state === 'running' ? (
        <IconButton icon={Pause} label="Pause Program" onClick={() => void control('pause', s)} />
      ) : (
        <IconButton icon={Play} label="Resume Program (F9)" className="wb-dbg-resume" disabled={!stopped} onClick={() => void control('continue', s)} />
      )}
      <IconButton icon={Square} label="Stop (Ctrl+F2)" className="wb-dbg-stop" disabled={!live} onClick={() => void stopSession(s)} />
      <span className="wb-dbg-sep" />
      <IconButton icon={RedoDot} label="Step Over (F8)" disabled={!stopped} onClick={() => void control('next', s)} />
      <IconButton icon={ArrowDownToDot} label="Step Into (F7)" disabled={!stopped} onClick={() => void control('stepIn', s)} />
      <IconButton icon={ArrowUpFromDot} label="Step Out (Shift+F8)" disabled={!stopped} onClick={() => void control('stepOut', s)} />
      <IconButton icon={TextCursorInput} label="Run to Cursor (Alt+F9)" disabled={!stopped} onClick={runTo} />
      <span className="wb-dbg-sep" />
      <IconButton icon={CircleDot} label="View Breakpoints (Ctrl+Shift+F8)" onClick={viewBreakpoints} />
      <IconButton icon={CircleSlash} label={bps.data?.muted ? 'Unmute Breakpoints' : 'Mute Breakpoints'} active={!!bps.data?.muted} onClick={() => void setMuted(s.projectId, !bps.data?.muted)} />
      <span className="wb-dbg-sep" />
      <IconButton icon={Sparkles} label="Ask agent about this stop" disabled={!stopped} onClick={() => void askAgentAboutStop(s)} />
      <span className="spacer" />
      {s.debuggeeTerminalId && (
        <IconButton icon={SquareTerminal} label="The program's terminal" onClick={() => openPanel({ kind: 'terminal', id: `terminal:${s.debuggeeTerminalId}`, title: s.name, params: { terminalId: s.debuggeeTerminalId } })} />
      )}
      {s.prelaunchTerminalId && !s.debuggeeTerminalId && (
        <IconButton icon={SquareTerminal} label="Output of the pre-launch step" onClick={() => openPanel({ kind: 'terminal', id: `terminal:${s.prelaunchTerminalId}`, title: `Before ${s.name}`, params: { terminalId: s.prelaunchTerminalId } })} />
      )}
      <span className={`wb-dbg-state ${stateTone(s)}`} title={s.error ?? s.stopped?.text ?? undefined}>
        {s.state === 'starting' && <Spinner size={11} />}
        {stateLabel(s)}
        {s.inContainer && <span className="wb-badge">container</span>}
      </span>
    </div>
  )
}

function FramesAndVariables({ s }: { s: DebugSession }) {
  const width = useDebugPrefs((p) => p.framesWidth)
  const setWidth = useDebugPrefs((p) => p.setFramesWidth)
  const start = useRef(width)
  return (
    <div className="wb-dbg-split">
      <div style={{ width, flex: 'none' }} className="wb-dbg-pane">
        <FramesView s={s} />
      </div>
      <Splitter direction="v" onResizeStart={() => (start.current = useDebugPrefs.getState().framesWidth)} onResize={(d) => setWidth(Math.max(180, Math.min(900, start.current + d)))} />
      <div className="wb-dbg-pane wb-grow">
        <VariablesView s={s} />
      </div>
    </div>
  )
}

export function DebugToolWindow({ projectId }: { projectId: string | null }) {
  const sessionsMap = useDebug((s) => s.sessions)
  const activeMap = useDebug((s) => s.active)
  const output = useDebug((s) => s.output)
  const tab = useDebugPrefs((p) => p.tab)
  const setTab = useDebugPrefs((p) => p.setTab)
  const bps = useBreakpoints(projectId)
  const list = useMemo(() => sessionsOf(sessionsMap, projectId), [sessionsMap, projectId])
  const s = useMemo(() => activeSession({ sessions: sessionsMap, active: activeMap }, projectId), [sessionsMap, activeMap, projectId])
  if (!projectId) return <EmptyState title="No project selected" />
  const bpCount = (bps.data?.breakpoints.length ?? 0) + (bps.data?.functionBreakpoints.length ?? 0)
  // A remembered Peripherals tab shows the first tab while this session has no register map.
  const shown: DebugTab = (tab === 'peripherals' && !s?.peripherals) || (tab === 'live' && !s?.live) ? 'frames' : tab
  const tabs: { id: DebugTab; label: string; badge?: ReactNode }[] = [
    { id: 'frames', label: s ? 'Threads & Variables' : 'Start' },
    { id: 'console', label: 'Console', badge: s && (output[s.id]?.length ?? 0) > 0 && tab !== 'console' ? <span className="wb-dbg-tabdot" /> : undefined },
    { id: 'breakpoints', label: 'Breakpoints', badge: bpCount ? <span className="wb-subtle wb-small"> {bpCount}</span> : undefined },
    // The chip's register map: only for a session whose configuration names an SVD file.
    ...(s?.peripherals ? [{ id: 'peripherals' as const, label: 'Peripherals' }] : []),
    // Variables read from the running program: only where the debug server has a Tcl port (OpenOCD).
    ...(s?.live ? [{ id: 'live' as const, label: 'Live' }] : []),
  ]
  return (
    <div className="wb-dbg">
      <div className="wb-dbg-head">
        {list.length > 0 ? <SessionTabs list={list} active={s} projectId={projectId} /> : <span className="wb-dbg-head-title wb-muted wb-small">No debug session</span>}
        <span className="wb-grow" />
        {s && remoteChip(s) && (
          <span className="wb-badge wb-dbg-remote" title={`Remote target: gdb is connected to ${s.remote?.target ?? 'the debug server'}`}>
            <Cpu size={11} /> {remoteChip(s)}
          </span>
        )}
        <ConfigPicker projectId={projectId} compact={!!s && isLive(s)} />
      </div>
      {/* CLion: the view tabs and the session's toolbar share one row. */}
      <div className="wb-dbg-bar">
        <Tabs tabs={tabs} value={shown} onChange={setTab} />
        {s && <span className="wb-dbg-sep" />}
        {s && <SessionToolbar s={s} />}
      </div>
      {s?.error && <div className="wb-dbg-error">{s.error}</div>}
      <div className="wb-dbg-body">
        {shown === 'frames' && (s ? <FramesAndVariables s={s} /> : <StartView projectId={projectId} />)}
        {shown === 'console' && (s ? <ConsoleView s={s} /> : <EmptyState title="No debug session">Output of the program and the debugger shows here.</EmptyState>)}
        {shown === 'breakpoints' && <BreakpointsView projectId={projectId} />}
        {shown === 'peripherals' && s && <PeripheralsView s={s} />}
        {shown === 'live' && s && <LiveWatchView s={s} />}
      </div>
    </div>
  )
}
