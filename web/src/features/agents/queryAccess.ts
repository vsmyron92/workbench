// Access to the ['terminals'] cache from outside React (palette commands). The
// TerminalsSync provider registers the app's QueryClient here.

import type { QueryClient } from '@tanstack/react-query'
import { qk } from '@/api/queries'
import type { ProjectSummary, TerminalInfo } from '@/api/types'
import { upsertTerminal } from './lib/sessions'

let client: QueryClient | null = null

export function setQueryClient(c: QueryClient | null) {
  client = c
}

export function cachedTerminals(): TerminalInfo[] | undefined {
  return client?.getQueryData<TerminalInfo[]>(qk.terminals)
}

/** The ids of the projects listed (the project switcher's), from the ['projects'] cache. */
export function cachedProjectIds(): string[] {
  return (client?.getQueryData<ProjectSummary[]>(qk.projects) ?? []).map((p) => p.id)
}

/** Put a terminal a request returned into the cache at once (its event follows). */
export function updateCachedTerminal(t: TerminalInfo) {
  client?.setQueryData<TerminalInfo[]>(qk.terminals, (old) => (old ? upsertTerminal(old, t) : old))
}
