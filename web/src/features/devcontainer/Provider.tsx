// Mounted once: keeps the dev container caches fresh from `devcontainer.state`, hosts
// the Start/Rebuild confirmation and the scaffold dialog, and answers the shell's
// dev container actions (Apps tool window, phone Apps tab).

import { useEffect, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { qk } from '@/api/queries'
import { useEvent } from '@/api/events'
import { setDevcontainerHandler } from '@/shell/devcontainerBridge'
import { openContainerShell, openDevcontainerPanel, setDcQueryClient, stopContainer } from './api'
import { openStartDialog, StartDialogHost } from './ConfirmDialog'
import { ScaffoldDialogHost } from './ScaffoldDialog'

export function DevcontainerProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  useEffect(() => {
    setDcQueryClient(qc)
    setDevcontainerHandler((pid, action) => {
      switch (action) {
        case 'start':
          return openStartDialog(pid)
        case 'rebuild':
          return openStartDialog(pid, true)
        case 'stop':
          return void stopContainer(pid)
        case 'shell':
          return void openContainerShell(pid)
        default:
          return openDevcontainerPanel(pid)
      }
    })
    return () => {
      setDcQueryClient(null)
      setDevcontainerHandler(null)
    }
  }, [qc])

  useEvent<{ projectId: string }>('devcontainer.state', (ev) => {
    const pid = ev.projectId ?? ev.data?.projectId
    void qc.invalidateQueries({ queryKey: qk.projects })
    if (pid) {
      void qc.invalidateQueries({ queryKey: ['devcontainer', pid] })
      // Where runs go (and their badges) follows the container.
      void qc.invalidateQueries({ queryKey: ['apps', 'runs', pid] })
    }
  })

  return (
    <>
      {children}
      <StartDialogHost />
      <ScaffoldDialogHost />
    </>
  )
}
