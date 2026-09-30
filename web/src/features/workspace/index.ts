// Feature slice: workspace. Owned by the workspace slice — see docs/ARCHITECTURE.md.
// Mr. Mak-style deliverable cards (adapted from Mr. Mak Workspace, MIT): the
// 'workspace.home' grid, the 'card' panel with its step viewers, the 'workspace'
// tool window, palette commands and the phone's 'workspace' tab.

import { lazy } from 'react'
import { LayoutGrid, Plus, Search, SquareKanban, Trash2 } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import { openHome, openTrash } from './actions'
import { HOME } from './logic'
import { WorkspaceProvider } from './Provider'
import { useWsUi } from './store'
import './workspace.css'

const HomePanel = lazy(() => import('./HomePanel'))
const CardPanel = lazy(() => import('./CardPanel'))
const WorkspaceToolWindow = lazy(() => import('./ToolWindow'))
const MobileWorkspace = lazy(() => import('./MobileWorkspace'))

const feature: FeatureModule = {
  id: 'workspace',
  panels: {
    'workspace.home': { component: HomePanel, icon: LayoutGrid },
    card: { component: CardPanel, icon: SquareKanban },
  },
  // First on the stripe, above Files, and the dock's first tab: the cards are what a project opens on.
  toolWindows: [{ id: 'workspace', title: 'Workspace', icon: LayoutGrid, side: 'left', order: 5, component: WorkspaceToolWindow }],
  startPanel: { kind: 'workspace.home', id: 'workspace.home', title: 'Workspace' },
  commands: () => [
    {
      id: 'workspace.home',
      title: 'Workspace home',
      group: 'Start',
      icon: LayoutGrid,
      keywords: ['cards', 'deliverables', 'reports', 'workspace'],
      run: () => openHome(),
    },
    {
      id: 'workspace.newCard',
      title: 'New card…',
      group: 'Workspace',
      icon: Plus,
      keywords: ['workspace', 'deliverable', 'report', 'create'],
      run: (c) => useWsUi.getState().openNewCard(c.projectId ?? HOME),
    },
    {
      id: 'workspace.openCard',
      title: 'Open card…',
      group: 'Workspace',
      icon: Search,
      keywords: ['workspace', 'deliverable', 'report', 'find'],
      run: () => useWsUi.getState().setPicker(true),
    },
    {
      id: 'workspace.trash',
      title: 'Workspace Trash',
      group: 'Workspace',
      icon: Trash2,
      keywords: ['workspace', 'deleted cards', 'restore', 'undelete'],
      run: () => openTrash(),
    },
  ],
  mobileTabs: [
    {
      id: 'workspace',
      title: 'Workspace',
      icon: LayoutGrid,
      order: 25,
      component: MobileWorkspace,
      openPanel: (p) => {
        const { scope, cardId, step } = p.params
        if (p.kind === 'card' && typeof scope === 'string' && typeof cardId === 'string') {
          useWsUi.getState().setMobile({ scope, cardId, step: typeof step === 'number' ? step : undefined })
          return true
        }
        if (p.kind === 'workspace.home') {
          useWsUi.getState().setMobile(null)
          return true
        }
        return false
      },
    },
  ],
  providers: [WorkspaceProvider],
}

export default feature
