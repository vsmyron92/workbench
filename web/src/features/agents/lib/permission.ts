// Pure helpers for permission requests answered from Workbench (Claude Code sessions,
// server/src/terminals/permission.rs). No React — covered by permission.test.ts.

import type { PendingPermission, TerminalInfo } from '@/api/types'
import { isRunning } from './sessions'

/** The request Workbench can answer for this session now (running sessions only). */
export function pendingOf(t: TerminalInfo | null | undefined): PendingPermission | null {
  if (!t?.agent || !isRunning(t)) return null
  return t.agent.pendingPermission ?? null
}

/**
 * A device's answer. A deny without a message stops the turn, like "No" in the terminal
 * without feedback; with one, Claude reads it and goes on.
 */
export type PermissionAnswer = { decision: 'allow'; scope?: 'once' | 'session' } | { decision: 'deny'; message?: string }

/** The body of POST /api/agents/{terminalId}/permission. */
export function answerBody(p: Pick<PendingPermission, 'id'>, a: PermissionAnswer): Record<string, unknown> {
  if (a.decision === 'allow') return { id: p.id, decision: 'allow', scope: a.scope ?? 'once' }
  const message = a.message?.trim()
  return message ? { id: p.id, decision: 'deny', message } : { id: p.id, decision: 'deny' }
}

/** What to tell the user when an answer failed: a 409 means it was settled elsewhere. */
export function answerFailure(e: unknown): { level: 'info' | 'error'; message: string } {
  const status = (e as { status?: unknown } | null)?.status
  if (status === 409) return { level: 'info', message: 'Already answered in the terminal, or no longer waiting' }
  return { level: 'error', message: `Not answered: ${e instanceof Error ? e.message : String(e)}` }
}

/** "Permission to run `npm test`" → lead "Permission to run", code "npm test". */
export function summaryParts(summary: string): { lead: string; code: string | null; tail: string } {
  const m = /^([^`]*)`([^`]*)`(.*)$/s.exec(summary)
  if (!m) return { lead: summary, code: null, tail: '' }
  return { lead: m[1].trim(), code: m[2], tail: m[3].trim() }
}

/** Tools whose request names a file the summary already shows: their detail is the content. */
const FILE_TOOLS = new Set(['Edit', 'MultiEdit', 'Write', 'NotebookEdit', 'Read'])

/** Most a request may hold to be answered at a glance (a toast, a notification). */
export const GLANCE_CHARS = 300
export const GLANCE_LINES = 4

/** Whether the request's detail says more than its one-line summary shows. */
export function detailNeeded(p: PendingPermission): boolean {
  return !!p.detail && !p.summary.includes(p.detail)
}

/** Whether the one-line summary shows the whole request: nothing cut or masked anywhere. */
export function summaryShowsAll(p: PendingPermission): boolean {
  return p.complete === true && !detailNeeded(p)
}

/**
 * Whether the detail is shown open at first: what runs (a command, an MCP tool's
 * arguments) when the summary does not show it all; file contents stay folded.
 */
export function detailOpenAtFirst(p: PendingPermission): boolean {
  return detailNeeded(p) && !FILE_TOOLS.has(p.tool)
}

/**
 * Whether a one-tap Allow may be offered where only a few lines fit (the toast): the
 * request is whole (nothing cut or masked), and short enough to show there in full.
 */
export function oneTapAllow(p: PendingPermission): boolean {
  if (p.complete !== true) return false
  const text = glanceText(p)
  return text.length <= GLANCE_CHARS && text.split('\n').length <= GLANCE_LINES
}

/** What a one-tap surface shows: the whole request (its detail, else its summary). */
export function glanceText(p: PendingPermission): string {
  return p.detail || p.summary
}

/** Shorter lead for tight places (cards, phone rows): "Run", "Edit src/x.rs"… */
export function shortLead(lead: string): string {
  const s = lead.replace(/^Permission to /, '')
  return s.charAt(0).toUpperCase() + s.slice(1)
}
