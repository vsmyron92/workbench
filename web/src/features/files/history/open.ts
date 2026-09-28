// Opening Local History from anywhere (tree menu, editor menu, palette, Recent
// Changes) and putting labels.

import { openPanel, promptDialog, toast, toastError } from '@/shell/actions'
import { historyApi } from './api'
import { historyPanelId, historyTitle } from './model'

export interface LocalHistoryParams {
  projectId: string
  /** Project-relative file or folder (`''` with `dir`: the whole project). */
  path: string
  /** Changes of the files in a folder instead of one file's versions. */
  dir?: boolean
  /** Select this entry when opening. */
  id?: number
}

export function showLocalHistory(projectId: string, path: string, dir = false, id?: number) {
  const params: LocalHistoryParams = { projectId, path, dir }
  if (id !== undefined) params.id = id
  openPanel({
    kind: 'localHistory',
    id: historyPanelId(projectId, path, dir),
    title: historyTitle(path, dir),
    params: params as unknown as Record<string, unknown>,
  })
}

/** CLion's Recent Changes (Alt+Shift+C): the project's changes, newest first. */
export function showRecentChanges(projectId: string) {
  showLocalHistory(projectId, '', true)
}

/** Put Label…: a named point in the history of `path` (`''`: the whole project). */
export async function putLabel(projectId: string, path = '') {
  const label = await promptDialog({
    title: 'Put Label',
    label: path ? `A label in the local history of ${path}` : 'A label in the local history of the project',
    placeholder: 'Before the refactoring',
    confirmLabel: 'Put Label',
  })
  if (label === null || !label.trim()) return
  try {
    await historyApi.label(projectId, path, label.trim())
    toast('success', `Label “${label.trim()}” put`, { timeout: 2500 })
  } catch (e) {
    toastError(e, 'Could not put the label')
  }
}
