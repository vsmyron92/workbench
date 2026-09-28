// Top bar attention pill and status bar counts.

import { useRef } from 'react'
import { Bot, BellRing } from 'lucide-react'
import { useTerminals } from '@/api/queries'
import { showToolWindow } from '@/shell/actions'
import { Spinner } from '@/ui'
import { openTerminal } from './api'
import { counts, nextAttention } from './lib/sessions'

/** "2 need you" — each click jumps to the next session waiting on the user. */
export function AttentionPill() {
  const { data } = useTerminals()
  const last = useRef<string | null>(null)
  const c = counts(data)
  if (!c.attention && !c.working) return null
  if (!c.attention) {
    return (
      <button className="wb-topbar-widget wb-ag-pill working" onClick={() => showToolWindow('agents')} title="Agent sessions at work">
        <Spinner size={11} />
        {c.working} working
      </button>
    )
  }
  return (
    <button
      className="wb-topbar-widget wb-ag-pill attention"
      title="Go to the next session that needs you"
      onClick={() => {
        const next = nextAttention(data, last.current)
        if (next) {
          last.current = next.id
          openTerminal(next)
        }
      }}
    >
      <BellRing size={13} />
      {c.attention} need{c.attention === 1 ? 's' : ''} you
    </button>
  )
}

export function AgentStatus() {
  const { data } = useTerminals()
  const c = counts(data)
  if (!c.running) return null
  const parts: string[] = []
  if (c.working) parts.push(`${c.working} working`)
  if (c.attention) parts.push(`${c.attention} need${c.attention === 1 ? 's' : ''} you`)
  if (!parts.length) parts.push(`${c.running} idle`)
  return (
    <button className={`wb-status-item wb-ag-status ${c.attention ? 'attention' : ''}`} onClick={() => showToolWindow('agents')} title="Agent sessions">
      {c.working ? <Spinner size={10} /> : <Bot size={13} />}
      {parts.join(' · ')}
    </button>
  )
}
