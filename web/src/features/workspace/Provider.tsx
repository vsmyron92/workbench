// Mounted once: keeps Workspace queries fresh from `workspace.changed`, opens cards
// an agent asked for on a phone, and hosts the feature's dialogs.

import { useEffect, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { subscribe, useEvent } from '@/api/events'
import { isMobileShell, openPanel } from '@/shell/actions'
import { wk } from './api'
import { WorkspaceDialogs } from './dialogs'
import { anyDirtyDraft } from './store'

interface Changed {
  scope?: string
  cardId?: string
}

/** One change often arrives as a few events (the write, then the watcher): refetch once. */
const COALESCE_MS = 150

export function WorkspaceProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined
    // 'scope' or 'scope\ncardId'; '' = everything.
    const pending = new Set<string>()
    const flush = () => {
      timer = undefined
      const keys = [...pending]
      pending.clear()
      void qc.invalidateQueries({ queryKey: wk.scopes })
      void qc.invalidateQueries({ queryKey: ['workspace', 'cards'] })
      if (keys.includes('')) {
        void qc.invalidateQueries({ queryKey: wk.all })
        return
      }
      for (const k of keys) {
        const tail = k.split('\n')
        for (const kind of ['card', 'files', 'content']) void qc.invalidateQueries({ queryKey: ['workspace', kind, ...tail] })
      }
    }
    const off = subscribe('workspace.changed', (ev) => {
      const d = (ev.data ?? {}) as Changed
      pending.add(!d.scope ? '' : d.cardId ? `${d.scope}\n${d.cardId}` : d.scope)
      if (!timer) timer = setTimeout(flush, COALESCE_MS)
    })
    // A trash changed: its scope's list and the All list.
    const offTrash = subscribe('workspace.trash', () => void qc.invalidateQueries({ queryKey: ['workspace', 'trash'] }))
    return () => {
      off()
      offTrash()
      clearTimeout(timer)
    }
  }, [qc])

  // Closing the tab drops this tab's session storage, and with it any unsaved draft.
  useEffect(() => {
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      if (!anyDirtyDraft()) return
      e.preventDefault()
      e.returnValue = ''
    }
    window.addEventListener('beforeunload', onBeforeUnload)
    return () => window.removeEventListener('beforeunload', onBeforeUnload)
  }, [])

  // The desktop opens panels from `ui.open` in the platform slice; a phone has no
  // dock there, so route our kinds to the Workspace tab.
  useEvent<{ panel?: string; id?: string; params?: Record<string, unknown>; title?: string }>('ui.open', (ev) => {
    const d = ev.data
    if (!isMobileShell() || document.visibilityState !== 'visible' || !d?.panel) return
    if (d.panel !== 'card' && d.panel !== 'workspace.home') return
    openPanel({ kind: d.panel, id: d.id, title: d.title, params: d.params ?? {} })
  })

  return (
    <>
      {children}
      <WorkspaceDialogs />
    </>
  )
}
