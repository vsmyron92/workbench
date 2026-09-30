// Feature slice: platform — settings, remote access and pairing, notifications,
// the MCP overview and the activity log. See docs/ARCHITECTURE.md.

import { lazy } from 'react'
import { Activity, Bell, Cable, Ellipsis, FileCode, KeyRound, MonitorSmartphone, QrCode, Settings } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import { useMobileHelp } from '@/features/help/mobile'
import { ActivityToolWindow } from './Activity'
import { MobileMore } from './MobileMore'
import { openPairDialog } from './PairDialog'
import { PlatformProvider } from './Provider'
import { openSettings, RemoteIndicator } from './StatusBar'

const SettingsPanel = lazy(() => import('./SettingsPanel').then((m) => ({ default: m.SettingsPanel })))

const feature: FeatureModule = {
  id: 'platform',
  panels: {
    settings: { component: SettingsPanel, icon: Settings },
  },
  toolWindows: [
    {
      id: 'activity',
      title: 'Activity',
      icon: Activity,
      side: 'bottom',
      order: 40,
      component: ActivityToolWindow,
    },
  ],
  commands: () => [
    { id: 'platform.settings', title: 'Settings', group: 'Workbench', shortcut: 'mod+,', icon: Settings, keywords: ['preferences', 'config'], run: () => openSettings() },
    { id: 'platform.pair', title: 'Pair a device…', group: 'Workbench', icon: QrCode, keywords: ['phone', 'remote', 'qr', 'mobile'], run: openPairDialog },
    { id: 'platform.remote', title: 'Remote access settings', group: 'Workbench', icon: MonitorSmartphone, run: () => openSettings('remote') },
    { id: 'platform.secrets', title: 'Secrets', group: 'Workbench', icon: KeyRound, keywords: ['token', 'chmod'], run: () => openSettings('secrets') },
    { id: 'platform.mcp', title: 'MCP servers and tools', group: 'Workbench', icon: Cable, run: () => openSettings('mcp') },
    {
      id: 'platform.notifications',
      title: 'Notification settings',
      group: 'Workbench',
      icon: Bell,
      keywords: ['push', 'phone', 'mobile', 'alerts'],
      run: () => openSettings('notifications'),
    },
    { id: 'platform.rawConfig', title: 'Edit config.toml', group: 'Workbench', icon: FileCode, keywords: ['raw', 'toml'], run: () => openSettings('raw') },
  ],
  statusbar: [RemoteIndicator],
  mobileTabs: [
    {
      id: 'more',
      title: 'More',
      icon: Ellipsis,
      order: 90,
      component: MobileMore,
      openPanel: (p) => {
        if (p.kind !== 'help') return false
        useMobileHelp.getState().show(typeof p.params.page === 'string' ? p.params.page : undefined)
        return true
      },
    },
  ],
  providers: [PlatformProvider],
}

export default feature
