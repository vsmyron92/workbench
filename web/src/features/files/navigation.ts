// Navigate Back / Forward: open the history entry (navHistory.ts) at its place.

import { navHistory } from './navHistory'
import { openFile } from './openers'

export function navigateBack() {
  const e = navHistory.back(Date.now())
  if (e) openFile({ projectId: e.projectId, path: e.path, line: e.line, column: e.column })
}

export function navigateForward() {
  const e = navHistory.forward(Date.now())
  if (e) openFile({ projectId: e.projectId, path: e.path, line: e.line, column: e.column })
}
