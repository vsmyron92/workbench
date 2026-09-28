// Invisible, always-mounted part of the files slice: keeps loaded tree folders
// fresh, opens files when an agent asks (`ui.open` from MCP), guards unsaved
// buffers on unload, and hosts the Go to File and Export as HTML dialogs.

import { useEffect, type ReactNode } from 'react'
import { useEvent } from '@/api/events'
import { openPanel } from '@/shell/actions'
import { shortcutsMayHandle } from '@/shell/paletteSearch'
import { hasDirtyBuffers, stashDrafts } from './buffers'
import { ExportDialogHost } from './export/ExportDialog'
import { editorPanelId, markdownPanelId, searchPanelId, tabTitle, basename } from './paths'
import { navigateBack, navigateForward } from './navigation'
import { BookmarkPopupHost } from './BookmarkPopups'
import { QuickOpenHost } from './QuickOpen'
import { RecentPopupHost } from './RecentPopups'
import { noteRecent } from './store'
import { startTreeSync } from './treeLoader'

interface UiOpen {
  panel: string
  params: Record<string, unknown>
  title?: string | null
  /** Stable panel id (added by the files slice's MCP tools). */
  id?: string
}

export function FilesProvider({ children }: { children?: ReactNode }) {
  useEffect(() => startTreeSync(), [])

  useEffect(() => {
    const beforeUnload = (e: BeforeUnloadEvent) => {
      if (hasDirtyBuffers()) {
        e.preventDefault()
        e.returnValue = ''
      }
    }
    window.addEventListener('beforeunload', beforeUnload)
    window.addEventListener('pagehide', stashDrafts)
    return () => {
      window.removeEventListener('beforeunload', beforeUnload)
      window.removeEventListener('pagehide', stashDrafts)
    }
  }, [])

  // Navigate Back / Forward outside editors (they bind these keys themselves): Alt+Left /
  // Alt+Right, which would otherwise leave Workbench (browser history), and the mouse's
  // back / forward buttons. Terminals keep their keys (Alt+Left moves a word in bash).
  useEffect(() => {
    const keys = (e: KeyboardEvent) => {
      if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight')) return
      if (!shortcutsMayHandle(e)) return
      e.preventDefault()
      if (e.key === 'ArrowLeft') navigateBack()
      else navigateForward()
    }
    const mouse = (e: MouseEvent) => {
      if (e.button !== 3 && e.button !== 4) return
      e.preventDefault()
      if (e.button === 3) navigateBack()
      else navigateForward()
    }
    const swallow = (e: MouseEvent) => {
      if (e.button === 3 || e.button === 4) e.preventDefault()
    }
    window.addEventListener('keydown', keys)
    window.addEventListener('mouseup', mouse)
    window.addEventListener('mousedown', swallow)
    return () => {
      window.removeEventListener('keydown', keys)
      window.removeEventListener('mouseup', mouse)
      window.removeEventListener('mousedown', swallow)
    }
  }, [])

  // Agents open files through MCP (`workbench_open_file` → `ui.open`). Only this
  // slice's panel kinds are handled here, always with their conventional ids.
  useEvent<UiOpen>('ui.open', (ev) => {
    const d = ev.data
    if (!d || typeof d.panel !== 'string' || !d.params) return
    const projectId = (d.params.projectId as string | null | undefined) ?? null
    const path = typeof d.params.path === 'string' ? d.params.path : ''
    if (d.panel === 'editor' && path) {
      noteRecent(projectId, path)
      openPanel({ kind: 'editor', id: d.id ?? editorPanelId(projectId, path), title: tabTitle(path), params: d.params })
    } else if (d.panel === 'markdown' && path) {
      openPanel({ kind: 'markdown', id: d.id ?? markdownPanelId(projectId, path), title: `${basename(path)} (preview)`, params: d.params })
    } else if (d.panel === 'search' && projectId) {
      openPanel({ kind: 'search', id: d.id ?? searchPanelId(projectId), title: 'Find in Files', params: d.params })
    }
  })

  return (
    <>
      {children}
      <QuickOpenHost />
      <RecentPopupHost />
      <BookmarkPopupHost />
      <ExportDialogHost />
    </>
  )
}
