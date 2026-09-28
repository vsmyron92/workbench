// Palette commands of the devcontainer feature.

import { Container, FilePlus2, Hammer, Play, Square, SquareTerminal } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import type { Command, CommandContext } from '@/shell/types'
import { openContainerShell, openDevcontainerPanel, stopContainer } from './api'
import { openStartDialog } from './ConfirmDialog'
import { openScaffoldDialog } from './ScaffoldDialog'

export function devcontainerCommands(ctx: CommandContext): Command[] {
  const services: Command = {
    id: 'devcontainer.services',
    title: 'Show Services',
    group: 'Tool windows',
    shortcut: 'alt+8',
    keywords: ['docker', 'containers', 'compose', 'images'],
    icon: Container,
    run: () => showToolWindow('services', 'bottom'),
  }
  const pid = ctx.projectId
  if (!pid) return [services]
  const s = ctx.project?.devcontainer ?? null
  const has = !!s && s.configs.length > 0
  const running = s?.state === 'running'
  const busy = s?.state === 'building'
  const group = 'Dev container'
  const keywords = ['devcontainer', 'docker', 'container']
  const out: Command[] = [
    services,
    { id: 'devcontainer.show', title: 'Show dev container', group, keywords, icon: Container, run: () => openDevcontainerPanel(pid), when: () => !!s },
    {
      id: 'devcontainer.start',
      title: running ? 'Attach to dev container…' : 'Start dev container…',
      group,
      keywords,
      icon: Play,
      run: () => openStartDialog(pid),
      when: () => has && !busy,
    },
    { id: 'devcontainer.stop', title: 'Stop dev container', group, keywords, icon: Square, run: () => void stopContainer(pid), when: () => running },
    { id: 'devcontainer.rebuild', title: 'Rebuild dev container…', group, keywords, icon: Hammer, run: () => openStartDialog(pid, true), when: () => has && !busy },
    {
      id: 'devcontainer.shell',
      title: 'Open dev container shell',
      group,
      keywords: [...keywords, 'terminal'],
      icon: SquareTerminal,
      run: () => void openContainerShell(pid),
      when: () => running,
    },
    {
      id: 'devcontainer.create',
      title: 'Create devcontainer.json…',
      group,
      keywords: [...keywords, 'scaffold', 'new'],
      icon: FilePlus2,
      run: () => openScaffoldDialog(pid),
      when: () => !has,
    },
  ]
  return out
}
