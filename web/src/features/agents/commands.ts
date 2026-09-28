// Palette commands of the agents feature.

import { BellRing, Bot, History, LayoutGrid, Radio, SquareTerminal } from 'lucide-react'
import { showToolWindow, toast, toastError } from '@/shell/actions'
import type { TerminalInfo } from '@/api/types'
import type { Command, CommandContext } from '@/shell/types'
import { openAgentsHome, openTerminal, terminalsApi } from './api'
import { nextAttention } from './lib/sessions'
import { cachedTerminals } from './queryAccess'
import { useAgentsUi } from './store'

/** Start a login shell in the project and show it in the Terminal tool window. */
export async function newShell(projectId: string | null): Promise<TerminalInfo | null> {
  try {
    const t = await terminalsApi.createShell(projectId)
    if (projectId) useAgentsUi.getState().setBottomTab(projectId, t.id)
    showToolWindow('terminal')
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
      id: 'agents.showTerminal',
      title: 'Show Terminal',
      group: 'Tool windows',
      shortcut: 'alt+f12',
      keywords: ['shell', 'console'],
      icon: SquareTerminal,
      run: () => showToolWindow('terminal', 'bottom'),
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
