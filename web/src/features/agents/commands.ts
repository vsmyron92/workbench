// Palette commands of the agents feature.

import { BellRing, Bot, History, LayoutGrid, Radio, SquareTerminal } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import type { TerminalInfo } from '@/api/types'
import type { Command, CommandContext } from '@/shell/types'
import { useUi } from '@/state/store'
import { openAgentsHome, openTerminal, terminalsApi } from './api'
import { nextAttention } from './lib/sessions'
import { cachedTerminals, updateCachedTerminal } from './queryAccess'
import { useAgentsUi } from './store'

/**
 * Start a login shell in the project and show it as a tab of the agents column.
 * `container`: in the project's dev container (true) or on the host (false); omitted: the project's default.
 */
export async function newShell(projectId: string | null, container?: boolean): Promise<TerminalInfo | null> {
  try {
    const t = await terminalsApi.createShell(projectId, undefined, container)
    updateCachedTerminal(t)
    openTerminal(t)
    return t
  } catch (e) {
    toastError(e, 'Could not start a shell')
    return null
  }
}

export function agentCommands(ctx: CommandContext): Command[] {
  const hasProject = !!ctx.projectId
  const open = useAgentsUi.getState().openDialog
  return [
    {
      id: 'agents.toggleWorkspaceWindow',
      title: 'Collapse or expand the workspace window',
      group: 'Tool windows',
      shortcut: 'alt+f12',
      keywords: ['shell', 'console', 'terminal', 'sessions', 'agents', 'full width', 'hide', 'sidebar'],
      icon: SquareTerminal,
      run: () => {
        const ui = useUi.getState()
        ui.setWorkOpen(!ui.workOpen)
      },
    },
    {
      id: 'agents.new',
      title: 'New agent session',
      group: 'Start',
      shortcut: 'mod+shift+a',
      keywords: ['claude', 'codex', 'kimi', 'gemini', 'aider', 'agent', 'session', 'prompt'],
      icon: Bot,
      when: () => hasProject,
      run: () => open({ kind: 'new' }),
    },
    {
      id: 'agents.home',
      title: 'Agents home',
      group: 'Start',
      keywords: ['sessions', 'claude', 'dashboard'],
      icon: LayoutGrid,
      run: () => openAgentsHome(),
    },
    {
      id: 'terminals.newShell',
      title: 'New shell',
      group: 'Start',
      keywords: ['terminal', 'bash', 'console'],
      icon: SquareTerminal,
      run: async (c) => {
        await newShell(c.projectId)
      },
    },
    {
      id: 'agents.resume',
      title: 'Resume agent session…',
      group: 'Agents',
      keywords: ['history', 'claude', 'continue'],
      icon: History,
      when: () => hasProject,
      run: () => open({ kind: 'resume' }),
    },
    {
      id: 'agents.remoteControl',
      title: 'Start Remote Control server',
      group: 'Agents',
      keywords: ['claude.ai', 'phone', 'remote'],
      icon: Radio,
      when: () => hasProject,
      run: () => open({ kind: 'remote' }),
    },
    {
      id: 'agents.nextAttention',
      title: 'Go to the next session that needs you',
      group: 'Agents',
      icon: BellRing,
      run: () => {
        const next = nextAttention(cachedTerminals(), null)
        if (next) openTerminal(next)
        else toast('info', 'No session is waiting for you')
      },
    },
  ]
}
