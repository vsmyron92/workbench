// Feature slice: data. Owned by the data slice: see docs/ARCHITECTURE.md ("Data viewer").
// Live measurements next to the agent session: logic analyzer captures, probe waveforms, power
// profiles. Agents fill series and make plots (MCP `data_*`); the browser draws them as lanes
// under one time axis while the capture runs.

import { lazy } from 'react'
import { ChartLine, Waves } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { openPlot } from './actions'
import { DataProvider } from './Provider'
import { useData } from './store'
import './data.css'

const PlotPanel = lazy(() => import('./PlotPanel').then((m) => ({ default: m.PlotPanel })))
const DataToolWindow = lazy(() => import('./DataToolWindow').then((m) => ({ default: m.DataToolWindow })))

const feature: FeatureModule = {
  id: 'data',
  panels: {
    // Lanes of series under one time axis, one saved layout per panel: `{projectId, plotId}`.
    'data.plot': { component: PlotPanel, icon: ChartLine },
  },
  toolWindows: [{ id: 'data', title: 'Data', icon: Waves, side: 'bottom', order: 27, component: DataToolWindow }],
  commands: (ctx) => {
    if (!ctx.projectId) return []
    const pid = ctx.projectId
    const plots = useData.getState().byProject[pid]?.plots ?? []
    return [
      {
        id: 'data.show',
        title: 'Show Data',
        group: 'Tool windows',
        keywords: ['measurements', 'plot', 'logic analyzer', 'power', 'oscilloscope', 'capture', 'waveform'],
        icon: Waves,
        run: () => showToolWindow('data', 'bottom'),
      },
      ...plots.slice(0, 20).map((p) => ({
        id: `data.plot.${p.id}`,
        title: `Open Plot: ${p.title}`,
        group: 'Data',
        icon: ChartLine,
        keywords: ['data', 'measurements'],
        run: () => openPlot(pid, p),
      })),
    ]
  },
  providers: [DataProvider],
}

export default feature
