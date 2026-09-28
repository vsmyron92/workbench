// Feature slice: devcontainer. Owned by the devcontainer slice — see docs/ARCHITECTURE.md
// ("Dev containers"). The project's dev container: config review, start/stop/rebuild,
// terminals and runs inside it. The Services tool window: every Docker container,
// compose project and image on this computer.
import { lazy } from 'react'
import { Container } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import { devcontainerCommands } from './commands'
import { DevcontainerProvider } from './Provider'
import { DevcontainerChip, DevcontainerStatus } from './widgets'
import './devcontainer.css'

const DevcontainerPanel = lazy(() => import('./Panel').then((m) => ({ default: m.DevcontainerPanel })))
const ServicesToolWindow = lazy(() => import('./services/ServicesToolWindow').then((m) => ({ default: m.ServicesToolWindow })))

const feature: FeatureModule = {
  id: 'devcontainer',
  panels: {
    devcontainer: { component: DevcontainerPanel, icon: Container },
  },
  toolWindows: [{ id: 'services', title: 'Services', icon: Container, side: 'bottom', order: 35, component: ServicesToolWindow }],
  commands: devcontainerCommands,
  topbar: [DevcontainerChip],
  statusbar: [DevcontainerStatus],
  providers: [DevcontainerProvider],
}

export default feature
