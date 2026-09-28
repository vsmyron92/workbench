// Feature slice: db. Owned by the db slice — see docs/ARCHITECTURE.md ("Database").
// The Database tool window (PostgreSQL data sources, schemas, tables, columns) and
// SQL consoles.
import { lazy } from 'react'
import { Database, SquareTerminal } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { DbProvider } from './Provider'
import './db.css'

const DatabaseToolWindow = lazy(() => import('./DatabaseToolWindow').then((m) => ({ default: m.DatabaseToolWindow })))
const ConsolePanel = lazy(() => import('./ConsolePanel').then((m) => ({ default: m.ConsolePanel })))

const feature: FeatureModule = {
  id: 'db',
  panels: {
    'db.console': { component: ConsolePanel, keepAlive: true, icon: SquareTerminal },
  },
  toolWindows: [{ id: 'database', title: 'Database', icon: Database, side: 'right', order: 60, component: DatabaseToolWindow }],
  commands: (ctx) =>
    ctx.projectId
      ? [
          {
            id: 'db.show',
            title: 'Show Database',
            group: 'Tool windows',
            keywords: ['sql', 'postgres', 'data sources', 'query console'],
            icon: Database,
            run: () => showToolWindow('database', 'right'),
          },
        ]
      : [],
  providers: [DbProvider],
}

export default feature
