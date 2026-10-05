// Feature slice: debug. Owned by the debug slice — see docs/ARCHITECTURE.md.
// A CLion-like debugger over the Debug Adapter Protocol: launch configurations,
// breakpoints in the editor gutter, stepping, frames, variables, watches, console.

import { lazy } from 'react'
import { Bug, ChartLine, FileCode } from 'lucide-react'
import type { FeatureModule } from '@/shell/types'
import { debugCommands } from './commands'
import { DebugProvider } from './DebugProvider'
import { DebugSourcePanel } from './SourcePanel'
import { DebugStatus } from './Topbar'
import './debug.css'

const DebugToolWindow = lazy(() => import('./DebugToolWindow').then((m) => ({ default: m.DebugToolWindow })))
const PlotPanel = lazy(() => import('./PlotPanel').then((m) => ({ default: m.PlotPanel })))

const feature: FeatureModule = {
  id: 'debug',
  // Frames outside the project (libraries, the standard library) and debugger-held
  // source, read-only: `{projectId, sessionId, path | sourceReference, name?, line?, column?, t?}`.
  panels: {
    'debug.source': { component: DebugSourcePanel, icon: FileCode },
    // Watched values drawn together, one saved configuration per panel: `{projectId, plotId}`.
    'debug.plot': { component: PlotPanel, icon: ChartLine },
  },
  toolWindows: [{ id: 'debug', title: 'Debug', icon: Bug, side: 'bottom', order: 25, component: DebugToolWindow }],
  commands: debugCommands,
  topbar: [DebugStatus],
  providers: [DebugProvider],
}

export default feature
