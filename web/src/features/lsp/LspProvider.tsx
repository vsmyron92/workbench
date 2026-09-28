// Mounted once: keeps the lsp caches fresh from `lsp.state` / `lsp.diagnostics`,
// installs the editor integration once Monaco is loaded for an editor, answers the
// servers' requests, and hosts the slice's popups and dialogs.

import { useEffect, useRef, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useEvent } from '@/api/events'
import { hasOpenBuffers, onBuffersChange } from '@/features/files/modelAccess'
import { lspKeys, setLspQueryClient, type Counts, type LspStatus, type LspWorkspaceEdit, type Progress, type ServerState } from './api'
import { ChooserHost } from './Chooser'
import { lsp } from './client'
import { EnableDialogHost, LogDialogHost, MessageRequestsHost } from './Dialogs'
import { FileStructureHost } from './FileStructure'
import { GotoSymbolHost } from './GotoSymbol'
import { RenameHost } from './RenameDialog'
import { usePopups } from './store'
import { applyWorkspaceEdit, reportApplied } from './workspaceEdit'
import './lsp.css'

export function LspProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  useEffect(() => {
    setLspQueryClient(qc)
    lsp.requestHandler = async (_pid, server, method, params) => {
      if (method === 'workspace/applyEdit') {
        const p = params as { label?: string; edit: LspWorkspaceEdit }
        const r = await applyWorkspaceEdit(p.edit)
        reportApplied(r, p.label || `${server} edit`)
        return r.applied ? { applied: true } : { applied: false, failureReason: r.failureReason }
      }
      if (method === 'window/showMessageRequest') {
        const p = params as { type?: number; message?: string; actions?: { title: string }[] }
        return new Promise((resolve) =>
          usePopups.getState().pushMessage({ server, level: p.type ?? 3, message: p.message ?? '', actions: (p.actions ?? []).slice(0, 6), resolve }),
        )
      }
      return null
    }
    // The editor integration needs Monaco, which loads with the first editor.
    const tryInstall = () => {
      if (hasOpenBuffers()) {
        void lsp.install()
        return true
      }
      return false
    }
    if (tryInstall()) return () => setLspQueryClient(null)
    const off = onBuffersChange(() => {
      if (tryInstall()) off()
    })
    return () => {
      off()
      setLspQueryClient(null)
    }
  }, [qc])

  // Progress and indexing ↔ ready patch the cache; other changes (a server started,
  // stopped or crashed; enabled; settings) refetch the status, coalesced.
  const timers = useRef(new Map<string, ReturnType<typeof setTimeout>>())
  const refetch = (pid: string) => {
    clearTimeout(timers.current.get(pid))
    timers.current.set(
      pid,
      setTimeout(() => {
        timers.current.delete(pid)
        void qc.invalidateQueries({ queryKey: lspKeys.status(pid) })
      }, 250),
    )
  }
  useEvent<{ server?: string; state?: ServerState; progress?: Progress | null; transition?: boolean }>('lsp.state', (ev) => {
    const pid = ev.projectId
    if (!pid) return
    const d = ev.data ?? {}
    const cached = qc.getQueryData<LspStatus>(lspKeys.status(pid))
    if (!d.server || !cached) return refetch(pid)
    const running = (s: ServerState) => s === 'indexing' || s === 'ready'
    const prev = cached.servers.find((x) => x.id === d.server)
    qc.setQueryData<LspStatus>(lspKeys.status(pid), (s) =>
      s ? { ...s, servers: s.servers.map((x) => (x.id === d.server ? { ...x, state: d.state ?? x.state, progress: d.progress ?? null } : x)) } : s,
    )
    if (!prev || !d.state || (d.state !== prev.state && !(running(d.state) && running(prev.state)))) refetch(pid)
  })

  useEvent<Counts>('lsp.diagnostics', (ev) => {
    const pid = ev.projectId
    if (!pid) return
    qc.setQueryData<LspStatus>(lspKeys.status(pid), (s) => (s ? { ...s, counts: ev.data } : s))
    void qc.invalidateQueries({ queryKey: lspKeys.diagnostics(pid) })
  })

  return (
    <>
      {children}
      <ChooserHost />
      <FileStructureHost />
      <GotoSymbolHost />
      <RenameHost />
      <EnableDialogHost />
      <LogDialogHost />
      <MessageRequestsHost />
    </>
  )
}
