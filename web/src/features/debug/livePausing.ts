// Reading values by stopping the program for a moment (debug servers without a side channel: J-Link, pyOCD, st-util, QEMU).
// It disturbs the program, so the user allows it per session, after being told what it does.

import { confirmDialog, toastError } from '@/shell/actions'
import { debugApi } from './api'
import type { DebugSession } from './types'

export const PAUSING_EXPLAINED =
  'This debug server has no side channel for reading memory while the program runs. The values can be read by stopping the program for a few milliseconds each time (every 100 ms or slower) and resuming it. Code that depends on timing (motor control, communication, watchdogs) can notice.'

/** Ask, then allow reading by pausing the program for this session (or take it away). Whether it was changed. */
export async function setPausing(s: Pick<DebugSession, 'projectId' | 'id'>, on: boolean): Promise<boolean> {
  if (on) {
    const ok = await confirmDialog({ title: 'Read the values by pausing the program?', message: PAUSING_EXPLAINED, confirmLabel: 'Allow for this session' })
    if (!ok) return false
  }
  try {
    await debugApi.livePausing(s.projectId, s.id, on)
    return true
  } catch (e) {
    toastError(e, on ? 'Could not allow reading by pausing' : 'Could not stop reading by pausing')
    return false
  }
}
