import { openPanel } from '@/shell/actions'

/** Open Help, on a page when a slug is given. Reopening focuses the one panel. */
export function openHelp(page?: string) {
  openPanel({ kind: 'help', id: 'help', title: 'Help', params: page ? { page } : {} })
}
