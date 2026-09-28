// Palette commands of the debugger, with CLion's keymap. The stepping keys are also
// taken by the provider before the editor while a session runs (Monaco binds F8,
// Shift+F8 and Ctrl+F2 itself); in the editor, Ctrl+F8, Alt+F9 and Ctrl+Shift+F8 are
// editor actions (they need the caret).

import { ArrowDownToDot, ArrowUpFromDot, Bug, BugPlay, CircleDot, CircleSlash, Pause, Play, Plug, RedoDot, RotateCw, Sparkles, Square, TextCursorInput } from 'lucide-react'
import { showToolWindow, toast } from '@/shell/actions'
import type { Command, CommandContext } from '@/shell/types'
import { askAgentAboutStop, control, currentSession, rerun, runToCursor, setMuted, startDebug, stopSession, toggleBreakpoint, viewBreakpoints } from './actions'
import { cachedBreakpoints, cachedConfigs } from './api'
import { lastFocusedEditor } from './editor'
import { isLive } from './logic'
import { openAttachPicker, openDebugPicker } from './store'

export function debugCommands(ctx: CommandContext): Command[] {
  const pid = ctx.projectId
  if (!pid) return []
  const group = 'Debug'
  const s = () => currentSession(pid)
  const configs = cachedConfigs(pid)?.configs ?? []
  const out: Command[] = [
    {
      id: 'debug.start',
      title: 'Debug',
      group,
      shortcut: 'shift+F9',
      icon: BugPlay,
      keywords: ['debugger', 'start debugging', 'gdb', 'lldb'],
      run: () => {
        if (!configs.length && !cachedConfigs(pid)) {
          openDebugPicker(pid)
          return
        }
        void startDebug(pid)
      },
    },
    { id: 'debug.pick', title: 'Debug…', group, shortcut: 'alt+shift+F9', icon: Bug, keywords: ['launch configuration', 'debugger'], run: () => openDebugPicker(pid) },
    { id: 'debug.attach', title: 'Attach to Process…', group, shortcut: 'mod+alt+F5', icon: Plug, keywords: ['pid', 'debugger'], run: () => openAttachPicker(pid) },
    { id: 'debug.resume', title: 'Resume Program', group, shortcut: 'F9', icon: Play, run: () => void control('continue', s()) },
    { id: 'debug.pause', title: 'Pause Program', group, icon: Pause, run: () => void control('pause', s()) },
    { id: 'debug.stepOver', title: 'Step Over', group, shortcut: 'F8', icon: RedoDot, run: () => void control('next', s()) },
    { id: 'debug.stepInto', title: 'Step Into', group, shortcut: 'F7', icon: ArrowDownToDot, run: () => void control('stepIn', s()) },
    { id: 'debug.stepOut', title: 'Step Out', group, shortcut: 'shift+F8', icon: ArrowUpFromDot, run: () => void control('stepOut', s()) },
    {
      id: 'debug.runToCursor',
      title: 'Run to Cursor',
      group,
      shortcut: 'alt+F9',
      icon: TextCursorInput,
      run: () => {
        const e = lastFocusedEditor()
        const pos = e?.editor.getPosition()
        if (e?.projectId && pos) void runToCursor(e.projectId, e.path, pos.lineNumber)
      },
    },
    { id: 'debug.stop', title: 'Stop Debugging', group, shortcut: 'mod+F2', icon: Square, run: () => void stopSession(s()) },
    { id: 'debug.rerun', title: 'Rerun Debug Session', group, icon: RotateCw, when: () => !!s()?.config, run: () => void rerun(s()) },
    {
      id: 'debug.toggleBreakpoint',
      title: 'Toggle Line Breakpoint',
      group,
      shortcut: 'mod+F8',
      icon: CircleDot,
      run: () => {
        const e = lastFocusedEditor()
        const pos = e?.editor.getPosition()
        if (e?.projectId && pos) void toggleBreakpoint(e.projectId, e.path, pos.lineNumber)
        else toast('info', 'Put the caret on a line of a project file first')
      },
    },
    { id: 'debug.viewBreakpoints', title: 'View Breakpoints', group, shortcut: 'mod+shift+F8', icon: CircleDot, run: viewBreakpoints },
    {
      id: 'debug.mute',
      title: cachedBreakpoints(pid)?.muted ? 'Unmute Breakpoints' : 'Mute Breakpoints',
      group,
      icon: CircleSlash,
      run: () => void setMuted(pid, !cachedBreakpoints(pid)?.muted),
    },
    {
      id: 'debug.askAgent',
      title: 'Ask Agent About This Stop',
      group,
      icon: Sparkles,
      when: () => s()?.state === 'stopped',
      run: () => {
        const cur = s()
        if (cur) void askAgentAboutStop(cur)
      },
    },
    { id: 'debug.show', title: 'Show Debug', group: 'Tool windows', shortcut: 'alt+5', icon: Bug, run: () => showToolWindow('debug', 'bottom') },
  ]
  // One command per launch configuration ("Debug server").
  for (const c of configs.slice(0, 60)) {
    out.push({ id: `debug.config:${c.name}`, title: `Debug ${c.name}`, group: 'Debug configurations', keywords: [c.adapterLabel ?? '', c.program ?? '', c.origin], icon: BugPlay, run: () => void startDebug(pid, c.name) })
  }
  const live = s()
  if (live && !isLive(live)) return out.filter((c) => !['debug.pause', 'debug.stop'].includes(c.id))
  return out
}
