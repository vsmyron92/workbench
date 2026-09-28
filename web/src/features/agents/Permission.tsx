// Permission requests answered from Workbench (Claude Code sessions): the request and
// Allow / Deny on the session card, the tab's header strip, the phone and the toast.
// Claude's own prompt in the terminal stays usable meanwhile; the first answer wins.
// The one-line summary is never all there is to read: the whole request (`detail`)
// and what "For session" allows are shown next to the buttons.

import { useState, type SyntheticEvent } from 'react'
import { Check, CheckCheck, ChevronDown, ChevronRight, EyeOff, MessageSquareText, ShieldQuestion, X } from 'lucide-react'
import type { PendingPermission, TerminalInfo } from '@/api/types'
import { promptDialog, toast } from '@/shell/actions'
import { Button, IconButton } from '@/ui'
import { terminalsApi } from './api'
import { answerBody, answerFailure, detailNeeded, detailOpenAtFirst, pendingOf, summaryParts, type PermissionAnswer } from './lib/permission'
import { updateCachedTerminal } from './queryAccess'

/** Inside clickable cards and rows: reading or answering must not also open the terminal. */
const keepInside = (e: SyntheticEvent) => e.stopPropagation()

const FIRST_WINS = "Claude's own prompt in the terminal stays usable: the first answer wins."

/** Send an answer. Returns whether it was taken (a 409 means it was settled elsewhere). */
export async function answerPermission(terminalId: string, p: Pick<PendingPermission, 'id'>, a: PermissionAnswer): Promise<boolean> {
  try {
    updateCachedTerminal(await terminalsApi.answerPermission(terminalId, answerBody(p, a)))
    return true
  } catch (e) {
    const f = answerFailure(e)
    toast(f.level, f.message)
    return false
  }
}

/** Deny, telling Claude what to do instead (the turn goes on with that). */
export async function denyWithFeedback(terminalId: string, p: PendingPermission): Promise<boolean> {
  const message = await promptDialog({
    title: 'Deny and tell Claude what to do instead',
    label: p.summary,
    placeholder: 'e.g. Use the test database, not production',
    multiline: true,
    confirmLabel: 'Deny',
  })
  if (message === null) return false
  return answerPermission(terminalId, p, { decision: 'deny', message })
}

/** "Permission to run `cmd`" with the command set as code. */
export function PermissionSummary({ p }: { p: PendingPermission }) {
  const { lead, code, tail } = summaryParts(p.summary)
  return (
    <span className="wb-ag-perm-summary" title={p.summary}>
      {lead}
      {code !== null && (
        <>
          {' '}
          <code>{code}</code>
        </>
      )}
      {tail && ` ${tail}`}
    </span>
  )
}

/** Allow / Allow for this session / Deny / Deny with feedback. */
export function PermissionActions({ t, p, compact }: { t: TerminalInfo; p: PendingPermission; compact?: boolean }) {
  const [busy, setBusy] = useState<string | null>(null)
  const run = async (key: string, f: () => Promise<boolean>) => {
    if (busy) return
    setBusy(key)
    try {
      await f()
    } finally {
      setBusy(null)
    }
  }
  return (
    <div className="wb-ag-perm-actions" onClick={keepInside} onKeyDown={keepInside}>
      <Button
        size="small"
        variant="primary"
        icon={Check}
        loading={busy === 'allow'}
        disabled={!!busy && busy !== 'allow'}
        title={`Allow once. ${FIRST_WINS}`}
        onClick={() => void run('allow', () => answerPermission(t.id, p, { decision: 'allow' }))}
      >
        Allow
      </Button>
      {p.sessionRule && (
        <Button
          size="small"
          icon={CheckCheck}
          loading={busy === 'session'}
          disabled={!!busy && busy !== 'session'}
          title={`Allow for the rest of this session: ${p.sessionRule}`}
          aria-describedby={`wb-perm-rule-${p.id}`}
          onClick={() => void run('session', () => answerPermission(t.id, p, { decision: 'allow', scope: 'session' }))}
        >
          {compact ? 'Session' : 'For session'}
        </Button>
      )}
      <Button
        size="small"
        icon={X}
        loading={busy === 'deny'}
        disabled={!!busy && busy !== 'deny'}
        title="Deny and stop the turn, like No in the terminal"
        onClick={() => void run('deny', () => answerPermission(t.id, p, { decision: 'deny' }))}
      >
        Deny
      </Button>
      <IconButton
        icon={MessageSquareText}
        size="small"
        label="Deny and tell Claude what to do instead…"
        disabled={!!busy}
        onClick={() => void run('feedback', () => denyWithFeedback(t.id, p))}
      />
    </div>
  )
}

/**
 * The whole request under its summary (open at first when what runs is not all in the
 * summary; file contents folded), and a note when part of it is masked or cut here.
 * Give it `key={p.id}`: each request starts folded or open on its own.
 */
export function PermissionDetail({ p }: { p: PendingPermission }) {
  const [open, setOpen] = useState(() => detailOpenAtFirst(p))
  const needed = detailNeeded(p)
  const partial = p.complete !== true
  if (!needed && !partial) return null
  return (
    <div className="wb-ag-perm-more" onClick={keepInside} onKeyDown={keepInside}>
      {needed && (
        <Button
          size="small"
          variant="ghost"
          icon={open ? ChevronDown : ChevronRight}
          className="wb-ag-perm-toggle"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          {open ? 'Full request' : 'Show the full request'}
        </Button>
      )}
      {needed && open && <pre className="wb-ag-perm-detail">{p.detail}</pre>}
      {partial && (
        <div className="wb-ag-perm-note">
          <EyeOff size={12} />
          <span>Part of it is masked or cut here: read it in the terminal before allowing.</span>
        </div>
      )}
    </div>
  )
}

/** What "For session" allows, as text (a tooltip never shows on touch screens). */
export function PermissionRule({ p }: { p: PendingPermission }) {
  if (!p.sessionRule) return null
  return (
    <div className="wb-ag-perm-rule" id={`wb-perm-rule-${p.id}`}>
      <span className="wb-muted">For session allows</span> <code>{p.sessionRule}</code>
    </div>
  )
}

/** The request and its answers (session card, phone row). Nothing when none is pending. */
export function PermissionRequest({ t, compact }: { t: TerminalInfo; compact?: boolean }) {
  const p = pendingOf(t)
  if (!p) return null
  return (
    <div className={compact ? 'wb-ag-perm compact' : 'wb-ag-perm'}>
      <div className="wb-ag-perm-head">
        <ShieldQuestion size={13} />
        <PermissionSummary p={p} />
      </div>
      <PermissionDetail key={p.id} p={p} />
      <PermissionActions t={t} p={p} compact={compact} />
      <PermissionRule p={p} />
    </div>
  )
}
