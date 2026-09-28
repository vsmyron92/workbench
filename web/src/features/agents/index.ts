// Feature slice: agents (terminals & agent sessions: Claude Code, Codex, Kimi, custom CLIs). Owned by the terminals
// slice — see docs/ARCHITECTURE.md.
import { Bot, LayoutGrid, SquareTerminal } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import './agents.css'
import { AgentsHome } from './AgentsHome'
import { agentCommands } from './commands'
import { MobileAgents } from './MobileAgents'
import { AgentDialogs, AttentionNotifier, FirstRunHome, TerminalsSync } from './providers'
import { useAgentsUi } from './store'
import { TerminalPanel } from './TerminalPanel'
import { AgentsBadge, AgentsToolWindow, TerminalToolWindow } from './ToolWindows'
import { AgentStatus, AttentionPill } from './widgets'

const feature: FeatureModule = {
  id: 'agents',
  panels: {
    'agents.home': { component: AgentsHome, icon: LayoutGrid },
    terminal: { component: TerminalPanel, keepAlive: true, icon: SquareTerminal },
  },
  toolWindows: [
    { id: 'agents', title: 'Agents', icon: Bot, side: 'left', order: 30, component: AgentsToolWindow, badge: AgentsBadge },
    { id: 'terminal', title: 'Terminal', icon: SquareTerminal, side: 'bottom', order: 10, component: TerminalToolWindow },
  ],
  commands: agentCommands,
  topbar: [AttentionPill],
  statusbar: [AgentStatus],
  mobileTabs: [
    {
      id: 'agents',
      title: 'Agents',
      icon: Bot,
      order: 10,
      component: MobileAgents,
      badge: AgentsBadge,
      // Phone: "Open" on an attention toast or after asking an agent shows that terminal.
      openPanel: (p) => {
        if (p.kind === 'terminal' && typeof p.params.terminalId === 'string') useAgentsUi.getState().setMobileTerminal(p.params.terminalId)
        else if (p.kind === 'agents.home') useAgentsUi.getState().setMobileTerminal(null)
        else return false
        return true
      },
    },
  ],
  providers: [TerminalsSync, AttentionNotifier, FirstRunHome, AgentDialogs],
}

export default feature
