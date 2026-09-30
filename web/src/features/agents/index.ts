// Feature slice: agents (terminals & agent sessions: Claude Code, Codex, Kimi, custom CLIs). Owned by the terminals
// slice — see docs/ARCHITECTURE.md.
import { Bot, LayoutGrid, SquareTerminal } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import './agents.css'
import { AgentColumn } from './AgentColumn'
import { AgentsHome } from './AgentsHome'
import { agentCommands } from './commands'
import { MobileAgents } from './MobileAgents'
import { AgentDialogs, AttentionNotifier, TerminalsSync } from './providers'
import { useAgentsUi } from './store'
import { TerminalPanel } from './TerminalPanel'
import { AgentsBadge, AgentStatus, AttentionPill } from './widgets'

const feature: FeatureModule = {
  id: 'agents',
  // On the desktop both kinds are tabs of the agents column, not of the dock (shell/actions `isColumnKind`).
  panels: {
    'agents.home': { component: AgentsHome, icon: LayoutGrid },
    terminal: { component: TerminalPanel, keepAlive: true, icon: SquareTerminal },
  },
  column: AgentColumn,
  commands: agentCommands,
  columnbar: [AttentionPill],
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
  providers: [TerminalsSync, AttentionNotifier, AgentDialogs],
}

export default feature
