// The avatar list and the terminals list from outside React: what the palette commands
// decide with (a command is listed and run without a component around it). The avatar
// provider registers the app's QueryClient here. Read only: the cache is written by the
// provider alone (sync.ts).

import type { QueryClient } from '@tanstack/react-query'
import { qk } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { avatarKeys } from './api'
import type { AvatarList } from './lib/types'

let client: Pick<QueryClient, 'getQueryData'> | null = null

/** Called by the avatar provider; `null` when it goes away. */
export function setQueryClient(c: Pick<QueryClient, 'getQueryData'> | null) {
  client = c
}

/** `GET /api/avatar` as the cache holds it; undefined before it answered. */
export function cachedAvatars(): AvatarList | undefined {
  return client?.getQueryData<AvatarList>(avatarKeys.all)
}

export function cachedTerminals(): TerminalInfo[] | undefined {
  return client?.getQueryData<TerminalInfo[]>(qk.terminals)
}
