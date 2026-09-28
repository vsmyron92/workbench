// Small presentational pieces shared by the agents views.

import { Asterisk, CodeXml, Container, Copy, ExternalLink, Moon, Radio, Sparkles, SquareTerminal, Wrench, type LucideIcon } from 'lucide-react'
import type { AgentProvider, TerminalInfo } from '@/api/types'
import { IconButton } from '@/ui'
import { copyText, openRemote } from './actions'
import { useAgentDefaults } from './api'
import { providerKindOf, providerLabel } from './lib/providers'
import { colorCss, stateLabel, tone } from './lib/sessions'

const PROVIDER_ICONS: Record<AgentProvider, LucideIcon> = {
  claude: Asterisk,
  codex: CodeXml,
  kimi: Moon,
  gemini: Sparkles,
  aider: Wrench,
  custom: SquareTerminal,
}

/** The icon of an agent CLI kind. */
export function ProviderIcon({ kind, size = 13 }: { kind: AgentProvider; size?: number }) {
  const Icon = PROVIDER_ICONS[kind] ?? SquareTerminal
  return <Icon size={size} className={`wb-ag-picon ${kind}`} aria-hidden />
}

/** Which CLI runs a session: icon + name (`compact`: the icon only, named in the tooltip). */
export function ProviderBadge({ t, compact }: { t: TerminalInfo; compact?: boolean }) {
  // Cached: the composer and cards share one query per project.
  const { data } = useAgentDefaults(t.projectId)
  const label = providerLabel(t, data?.providers)
  const kind = providerKindOf(t)
  return (
    <span className={compact ? 'wb-ag-pbadge compact' : 'wb-ag-pbadge'} title={`Runs in ${label}`}>
      <ProviderIcon kind={kind} size={compact ? 12 : 11} />
      {!compact && <span>{label}</span>}
    </span>
  )
}

/** The terminal's process runs in its project's dev container (`meta.inContainer`). */
export function ContainerBadge({ t, compact }: { t: TerminalInfo; compact?: boolean }) {
  if (t.meta?.inContainer !== true) return null
  const c = (t.meta.container ?? {}) as { name?: string; user?: string | null; folder?: string }
  const tip = `Runs in the dev container${c.name ? ` ${c.name}` : ''}${c.user ? ` as ${c.user}` : ''}${c.folder ? ` (${c.folder})` : ''}`
  return (
    <span className={compact ? 'wb-ag-ctr compact' : 'wb-ag-ctr'} title={tip} aria-label={tip}>
      <Container size={11} />
      {!compact && <span>container</span>}
    </span>
  )
}

/** Coloured dot for a terminal's state (animated while working). */
export function StateDot({ t, size = 8 }: { t: TerminalInfo; size?: number }) {
  return <span className={`wb-ag-dot ${tone(t)}`} style={{ width: size, height: size }} title={stateLabel(t)} />
}

/** State pill: dot + label. */
export function StateChip({ t }: { t: TerminalInfo }) {
  return (
    <span className={`wb-ag-chip ${tone(t)}`}>
      <span className={`wb-ag-dot ${tone(t)}`} />
      {stateLabel(t)}
    </span>
  )
}

/** The tab colour as a thin bar (or nothing). */
export function ColorBar({ color }: { color: string | null }) {
  const c = colorCss(color)
  return c ? <span className="wb-ag-colorbar" style={{ background: c }} /> : null
}

/** Remote Control link: opens claude.ai, with a copy button. */
export function RemoteLink({ url, compact }: { url: string; compact?: boolean }) {
  return (
    <span className="wb-ag-remote" title={url}>
      <button className="wb-ag-remote-open" onClick={() => openRemote(url)}>
        <Radio size={13} />
        {!compact && <span>Remote Control</span>}
        <ExternalLink size={11} />
      </button>
      <IconButton icon={Copy} size="small" label="Copy Remote Control link" onClick={() => void copyText(url, 'Remote Control link copied')} />
    </span>
  )
}
