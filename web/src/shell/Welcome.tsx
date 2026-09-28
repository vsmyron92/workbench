// Shown in the center area when no panels are open.

import { Command as CommandIcon } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { useUi } from '@/state/store'
import { Kbd } from '@/ui'
import { featureCommands } from './registry'
import { openPalette } from './CommandPalette'

export function Welcome() {
  const projectId = useUi((s) => s.projectId)
  const { data: projects } = useProjects()
  const project = projects?.find((p) => p.id === projectId) ?? null
  const commands = featureCommands({ projectId, project })
    .filter((c) => c.group === 'Start')
    .slice(0, 6)
  return (
    <div className="wb-welcome">
      <div className="wb-welcome-inner">
        <h1>{project ? project.name : 'Workbench'}</h1>
        {project && <div className="wb-muted mono wb-small">{project.root}</div>}
        <div className="wb-welcome-actions">
          {commands.map((c) => (
            <button key={c.id} className="wb-welcome-action" onClick={() => c.run({ projectId, project })}>
              {c.icon ? <c.icon size={18} /> : <CommandIcon size={18} />}
              <span className="wb-grow">{c.title}</span>
              {c.shortcut && <Kbd>{c.shortcut.replace('mod', 'Ctrl')}</Kbd>}
            </button>
          ))}
          <button className="wb-welcome-action" onClick={() => openPalette()}>
            <CommandIcon size={18} />
            <span className="wb-grow">All commands</span>
            <Kbd>Ctrl+K</Kbd>
          </button>
        </div>
      </div>
    </div>
  )
}
