// Feature slice: help — the user documentation. Markdown pages bundled with the app,
// shown in the 'help' panel (F1, the palette, the status bar) and in the phone's More tab.
// See docs/ARCHITECTURE.md.

import { lazy } from 'react'
import { CircleHelp } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import { openHelp } from './actions'
import { HelpButton } from './HelpButton'

const HelpPanel = lazy(() => import('./HelpPanel'))

const feature: FeatureModule = {
  id: 'help',
  panels: {
    help: { component: HelpPanel, icon: CircleHelp },
  },
  commands: () => [
    { id: 'help.open', title: 'Help', group: 'Workbench', shortcut: 'f1', icon: CircleHelp, keywords: ['documentation', 'docs', 'manual', 'guide', 'how to'], run: () => openHelp() },
    { id: 'help.remote', title: 'Help: remote access and phone', group: 'Workbench', icon: CircleHelp, keywords: ['tailscale', 'qr', 'pair', 'mobile', 'push'], run: () => openHelp('remote-access') },
    { id: 'help.config', title: 'Help: configuration', group: 'Workbench', icon: CircleHelp, keywords: ['config.toml', 'settings reference'], run: () => openHelp('configuration') },
  ],
  statusbar: [HelpButton],
}

export default feature
