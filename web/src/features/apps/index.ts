// Feature slice: apps. Owned by the apps slice — see docs/ARCHITECTURE.md.
// Run configurations, environments (health, version, logs, deploy) and live previews.
import { lazy } from 'react'
import { AppWindow, Globe, Play, Rocket } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import { AppPanel } from './AppPanel'
import { AppsEvents } from './AppsEvents'
import { AppsToolWindow } from './AppsToolWindow'
import { appsCommands } from './commands'
import { DeployDialogHost } from './DeployDialog'
import { HttpProvider } from './http/HttpProvider'
import { MobileApps } from './MobileApps'
import { RunToolWindow } from './RunToolWindow'
import { EnvPills, RunSelector, StatusbarApps } from './Topbar'
import './apps.css'

const HttpResponsePanel = lazy(() => import('./http/HttpResponsePanel').then((m) => ({ default: m.HttpResponsePanel })))

const feature: FeatureModule = {
  id: 'apps',
  panels: {
    app: { component: AppPanel, icon: AppWindow },
    httpResponse: { component: HttpResponsePanel, icon: Globe },
  },
  toolWindows: [
    { id: 'apps', title: 'Apps', icon: Rocket, side: 'right', order: 30, component: AppsToolWindow },
    { id: 'run', title: 'Run', icon: Play, side: 'bottom', order: 30, component: RunToolWindow },
  ],
  commands: appsCommands,
  topbar: [RunSelector, EnvPills],
  statusbar: [StatusbarApps],
  mobileTabs: [{ id: 'apps', title: 'Apps', icon: Rocket, order: 60, component: MobileApps }],
  providers: [AppsEvents, DeployDialogHost, HttpProvider],
}

export default feature
