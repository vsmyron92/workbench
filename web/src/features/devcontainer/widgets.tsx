// Top-bar chip and status-bar item for projects with a dev container: its state at a
// glance; a click opens the panel.

import { Container } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { Spinner, StatusDot } from '@/ui'
import { openDevcontainerPanel } from './api'
import { stateLabel, stateTone } from './logic'

function useSummary(projectId: string | null) {
  const { data } = useProjects()
  return projectId ? (data?.find((p) => p.id === projectId)?.devcontainer ?? null) : null
}

function tooltip(s: { state: Parameters<typeof stateLabel>[0]; inContainer: boolean; configs: string[] }) {
  return [
    `Dev container: ${stateLabel(s.state)}`,
    s.inContainer ? 'New shells and runs start inside it' : s.state === 'running' ? 'Shells and runs use the host' : null,
    s.configs.join(', '),
  ]
    .filter(Boolean)
    .join('\n')
}

export function DevcontainerChip({ projectId }: { projectId: string | null }) {
  const s = useSummary(projectId)
  if (!projectId || !s) return null
  const tone = stateTone(s.state)
  return (
    <button className={`wb-topbar-widget wb-dc-chip ${tone}`} title={tooltip(s)} onClick={() => openDevcontainerPanel(projectId)} aria-label={`Dev container: ${stateLabel(s.state)}`}>
      <Container size={14} />
      {s.state === 'building' ? <Spinner size={10} /> : <StatusDot tone={tone} />}
      <span className="wb-dc-chip-label">{s.state === 'running' ? (s.inContainer ? 'In container' : 'Container') : stateLabel(s.state)}</span>
    </button>
  )
}

export function DevcontainerStatus({ projectId }: { projectId: string | null }) {
  const s = useSummary(projectId)
  if (!projectId || !s || s.state === 'none') return null
  return (
    <button className="wb-status-item" title={tooltip(s)} onClick={() => openDevcontainerPanel(projectId)}>
      <Container size={12} />
      {s.state === 'building' ? <Spinner size={9} /> : <StatusDot tone={stateTone(s.state)} />}
      {s.inContainer ? 'in container' : `container ${stateLabel(s.state).toLowerCase()}`}
    </button>
  )
}
