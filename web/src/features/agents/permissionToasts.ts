// The permission-request toasts on screen, so they go away once their request is settled
// or their session is on screen (where the same Allow / Deny are shown).

import type { TerminalInfo } from '@/api/types'
import { dismissToast } from '@/shell/actions'

const shown = new Map<string, { toastId: number; terminalId: string; since: number }>()

export function hasPermissionToast(permissionId: string): boolean {
  return shown.has(permissionId)
}

export function notePermissionToast(permissionId: string, toastId: number, terminalId: string, since: number) {
  shown.set(permissionId, { toastId, terminalId, since })
}

/** The session is on screen: its request toasts are redundant. */
export function dismissPermissionToasts(terminalId: string) {
  for (const [id, s] of shown) {
    if (s.terminalId !== terminalId) continue
    dismissToast(s.toastId)
    shown.delete(id)
  }
}

/**
 * Requests queue in order: one is settled once its session waits on none, or on a later one.
 * Dismiss the toasts of settled requests.
 */
export function settlePermissionToasts(t: TerminalInfo) {
  const current = t.status === 'exited' ? null : (t.agent?.pendingPermission ?? null)
  for (const [id, s] of shown) {
    if (s.terminalId !== t.id || current?.id === id) continue
    if (!current || current.since > s.since) {
      dismissToast(s.toastId)
      shown.delete(id)
    }
  }
}
