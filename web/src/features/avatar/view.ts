// What the avatar overlay needs to know of a session, taken from the terminals list with
// the agents feature's own helpers, so that the avatar agrees with the tab, the card and
// the pill. One small object per session, kept the same object while nothing in it
// changes: the terminals list is replaced with every update of any session, and what is
// drawn from a session must not be drawn again for another one's news.

import type { AgentProvider, TerminalInfo } from '@/api/types'
import { pendingOf } from '@/features/agents/lib/permission'
import { composeBlocked, isRunning, stateLabel, tone, type Tone } from '@/features/agents/lib/sessions'
import type { Session } from './lib/stage'

export interface SessionView {
  id: string
  projectId: string | null
  /** The session's live title. */
  title: string
  provider: AgentProvider
  tone: Tone
  /** The tab's own word for its state ("Working", "Needs permission"…). */
  label: string
  /** It needs a permission or an answer: its avatar is quiet (rule 6). */
  blocked: boolean
  /** It has a permission request Workbench can answer (the strip in its terminal). */
  pending: boolean
  /** Why nothing can be pasted into it now, before a waiting call is taken into account. */
  refuses: 'dialog' | 'starting' | null
  working: boolean
  running: boolean
  /** It was started with the Avatar option. */
  withOption: boolean
  createdAt: number
}

const PROVIDERS: ReadonlySet<string> = new Set(['claude', 'codex', 'kimi', 'gemini', 'aider', 'custom'])

export function sessionView(t: TerminalInfo): SessionView {
  const state = tone(t)
  const provider = t.agent?.provider
  return {
    id: t.id,
    projectId: t.projectId ?? null,
    title: typeof t.title === 'string' ? t.title : '',
    provider: typeof provider === 'string' && PROVIDERS.has(provider) ? provider : 'custom',
    tone: state,
    label: stateLabel(t),
    blocked: state === 'attention',
    pending: pendingOf(t) !== null,
    refuses: composeBlocked(t) === null ? null : t.agent?.state === 'starting' ? 'starting' : 'dialog',
    working: state === 'working',
    running: isRunning(t),
    withOption: t.meta?.avatar === true,
    createdAt: typeof t.createdAt === 'number' ? t.createdAt : 0,
  }
}

const KEYS: readonly (keyof SessionView)[] = ['id', 'projectId', 'title', 'provider', 'tone', 'label', 'blocked', 'pending', 'refuses', 'working', 'running', 'withOption', 'createdAt']
const sameView = (a: SessionView, b: SessionView): boolean => KEYS.every((k) => a[k] === b[k])

/**
 * The views of the agent sessions in `terminals`, by terminal id. A session whose view
 * did not change keeps its object, and the map itself comes back when none did.
 */
export function sessionViews(prev: ReadonlyMap<string, SessionView>, terminals: readonly TerminalInfo[] | undefined): ReadonlyMap<string, SessionView> {
  const next = new Map<string, SessionView>()
  let changed = false
  for (const t of terminals ?? []) {
    if (t?.kind !== 'agent' || typeof t.id !== 'string') continue
    const view = sessionView(t)
    const had = prev.get(t.id)
    if (had && sameView(had, view)) next.set(t.id, had)
    else {
      next.set(t.id, view)
      changed = true
    }
  }
  return changed || next.size !== prev.size ? next : prev
}

/** A terminal as the stage decides with it (lib/stage.ts). */
export function sessionOf(t: TerminalInfo): Session {
  return { id: t.id, projectId: t.projectId ?? null, agent: t.kind === 'agent', open: t.open === true, running: isRunning(t), pinned: t.pinned === true, order: typeof t.order === 'number' ? t.order : 0, createdAt: typeof t.createdAt === 'number' ? t.createdAt : 0 }
}
