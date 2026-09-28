// The editor's "Enable code intelligence" banner: once per project, when a language
// server could handle the file and code intelligence is off; or, when the server is
// not installed, how to install it (once per server and project).

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Braces, PackageX, X } from 'lucide-react'
import { Button, IconButton } from '@/ui'
import { useLspStatus } from './hooks'
import { serverFor } from './logic'
import { useDismissed, usePopups } from './store'

export function EnableBanner({ projectId, path, language, onHeight }: { projectId: string; path: string; language: string; onHeight: (h: number) => void }) {
  const st = useLspStatus(projectId)
  const dismissed = useDismissed((s) => s.map)
  const dismiss = useDismissed((s) => s.dismiss)
  const [hint, setHint] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const s = st.data
  const server = s ? serverFor(s, path, language) : null
  let kind: 'enable' | 'missing' | null = null
  if (server?.available && !s?.enabled && !dismissed[projectId]) kind = 'enable'
  else if (server && !server.available && !dismissed[`${projectId}:missing:${server.id}`]) kind = 'missing'

  useLayoutEffect(() => {
    onHeight(kind && ref.current ? ref.current.offsetHeight : 0)
  })
  // The text wraps as the editor narrows: keep the space above the first line in step.
  useEffect(() => {
    const el = ref.current
    if (!kind || !el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => onHeight(el.offsetHeight))
    ro.observe(el)
    return () => ro.disconnect()
  }, [kind, onHeight])

  if (!kind || !server) return null
  if (kind === 'enable') {
    return (
      <div ref={ref} className="lsp-banner" role="status">
        <Braces size={14} className="lsp-banner-icon" />
        <span className="wb-grow lsp-banner-text">
          Code intelligence is off for this project: <b>{server.label}</b> adds navigation, completion and diagnostics, and runs project code.
        </span>
        <Button size="small" variant="primary" onClick={() => usePopups.getState().set({ enable: { projectId, serverId: server.id } })}>
          Enable…
        </Button>
        <Button size="small" variant="ghost" onClick={() => dismiss(projectId)}>
          Not Now
        </Button>
      </div>
    )
  }
  return (
    <div ref={ref} className="lsp-banner warning" role="status">
      <PackageX size={14} className="lsp-banner-icon" />
      <span className="wb-grow lsp-banner-text" title={server.missing ?? undefined}>
        {hint ? (
          <>
            Install {server.label}: <code className="lsp-code">{server.installHint}</code>
          </>
        ) : (
          <>
            No language server for this file: <b>{server.label}</b> is not installed.
          </>
        )}
      </span>
      {!hint && (
        <Button size="small" onClick={() => setHint(true)}>
          How to Install
        </Button>
      )}
      <IconButton icon={X} size="small" label="Dismiss" onClick={() => dismiss(`${projectId}:missing:${server.id}`)} />
    </div>
  )
}
