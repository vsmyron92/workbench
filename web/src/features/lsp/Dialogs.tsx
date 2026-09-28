// The Enable dialog (what enabling means, which servers would run, where), a server's
// log, and the servers' own questions (`window/showMessageRequest`).

import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { AlertTriangle, Braces, RefreshCw } from 'lucide-react'
import { Badge, Button, ErrorBox, IconButton, Loading, Modal, Select, TimeAgo } from '@/ui'
import { toast, toastError } from '@/shell/actions'
import { useProjects } from '@/api/queries'
import { lspApi, type Mode } from './api'
import { lsp } from './client'
import { useLspStatus } from './hooks'
import { STATE_LABEL } from './logic'
import { usePopups } from './store'

export function EnableDialogHost() {
  const e = usePopups((s) => s.enable)
  if (!e) return null
  return <EnableDialog projectId={e.projectId} serverId={e.serverId} />
}

function EnableDialog({ projectId, serverId }: { projectId: string; serverId?: string }) {
  const st = useLspStatus(projectId)
  const { data: projects } = useProjects()
  const project = projects?.find((p) => p.id === projectId)
  const [mode, setMode] = useState<Mode>('auto')
  const [busy, setBusy] = useState(false)
  const close = () => usePopups.getState().set({ enable: null })
  const s = st.data
  useEffect(() => {
    if (s) setMode(s.mode)
  }, [s])
  const servers = useMemo(() => {
    if (!s) return []
    const list = s.servers.filter((x) => x.enabled && !x.disabledHere && x.available && (x.relevant || x.id === serverId))
    return list.length ? list : s.servers.filter((x) => x.id === serverId)
  }, [s, serverId])
  const enable = async () => {
    setBusy(true)
    try {
      const r = await lspApi.enable(projectId, mode)
      lsp.statusChanged(r)
      toast('success', `Code intelligence is on for ${project?.name ?? projectId}`, { timeout: 3000 })
      close()
    } catch (e) {
      toastError(e, 'Could not enable code intelligence')
      setBusy(false)
    }
  }
  const container = s?.devcontainer && s.devcontainer.state !== 'none'
  return (
    <Modal
      title={`Enable code intelligence for ${project?.name ?? projectId}?`}
      onClose={close}
      footer={
        <>
          <Button onClick={close}>Cancel</Button>
          <Button variant="primary" icon={Braces} loading={busy} disabled={!s} onClick={() => void enable()}>
            Enable
          </Button>
        </>
      }
    >
      {st.error ? (
        <ErrorBox error={st.error} />
      ) : !s ? (
        <Loading />
      ) : (
        <div className="lsp-enable">
          <div className="lsp-enable-warn">
            <AlertTriangle size={16} />
            <div>
              Language servers <b>run code from this project</b> to understand it: build scripts and procedural macros (rust-analyzer runs cargo),
              the project&apos;s TypeScript configuration and plugins, <code className="lsp-code">go list</code>, Python environments. Enable it for
              projects you trust, as you would before building them.
            </div>
          </div>
          <div className="wb-small wb-muted">A server starts when you open a file of its language, and stops after a while without one.</div>
          {servers.length > 0 && (
            <div className="lsp-enable-list">
              {servers.map((x) => (
                <div key={x.id} className="lsp-enable-row">
                  <b>{x.label}</b>
                  <code className="lsp-code wb-ellipsis" title={x.command}>
                    {x.command}
                  </code>
                  {x.side === 'container' && <Badge tone="accent">container</Badge>}
                </div>
              ))}
            </div>
          )}
          {container && (
            <label className="lsp-enable-mode">
              <span>Run servers</span>
              <Select value={mode} onChange={(e) => setMode(e.target.value as Mode)}>
                <option value="auto">In the dev container when it runs, else on this computer</option>
                <option value="container">Only in the dev container</option>
                <option value="host">On this computer</option>
              </Select>
            </label>
          )}
          <div className="wb-small wb-subtle">Commands come from Workbench&apos;s config.toml ([lsp.servers.*]), never from the repository.</div>
        </div>
      )}
    </Modal>
  )
}

// ---------------------------------------------------------------- logs

export function LogDialogHost() {
  const l = usePopups((s) => s.logs)
  if (!l) return null
  return <LogDialog projectId={l.projectId} serverId={l.serverId} />
}

function LogDialog({ projectId, serverId }: { projectId: string; serverId: string }) {
  const q = useQuery({ queryKey: ['lsp', 'log', projectId, serverId], queryFn: () => lspApi.log(projectId, serverId, 2000), refetchInterval: 3000 })
  const st = useLspStatus(projectId)
  const server = st.data?.servers.find((s) => s.id === serverId)
  const box = useRef<HTMLDivElement>(null)
  const stick = useRef(true)
  useEffect(() => {
    const el = box.current
    if (el && stick.current) el.scrollTop = el.scrollHeight
  }, [q.data])
  const close = () => usePopups.getState().set({ logs: null })
  return (
    <Modal
      wide
      title={
        <span className="wb-row" style={{ gap: 8 }}>
          {server?.label ?? serverId} log
          {server && <Badge tone={server.state === 'crashed' || server.state === 'failed' ? 'danger' : undefined}>{STATE_LABEL[server.state]}</Badge>}
        </span>
      }
      onClose={close}
      footer={
        <>
          <span className="wb-grow wb-small wb-muted">{server?.running ?? server?.command}</span>
          <IconButton icon={RefreshCw} label="Refresh" onClick={() => void q.refetch()} />
          <Button onClick={close}>Close</Button>
        </>
      }
    >
      {q.error ? (
        <ErrorBox error={q.error} />
      ) : !q.data ? (
        <Loading />
      ) : !q.data.lines.length ? (
        <div className="wb-muted wb-small">Nothing logged yet: the server has not run in this session.</div>
      ) : (
        <div
          ref={box}
          className="lsp-log"
          onScroll={(e) => {
            const el = e.currentTarget
            stick.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 8
          }}
        >
          {q.data.lines.map((l) => (
            <div key={l.seq} className={`lsp-log-line ${l.stream}`}>
              <span className="lsp-log-ts">{new Date(l.ts).toLocaleTimeString()}</span>
              <span className="lsp-log-text">{l.text}</span>
            </div>
          ))}
        </div>
      )}
      {server?.startedAt && (
        <div className="wb-small wb-subtle" style={{ marginTop: 6 }}>
          Started <TimeAgo time={server.startedAt} />
          {server.pid ? ` · pid ${server.pid}` : ''}
        </div>
      )}
    </Modal>
  )
}

// ---------------------------------------------------------------- server questions

export function MessageRequestsHost() {
  const list = usePopups((s) => s.messages)
  const m = list[0]
  if (!m) return null
  const answer = (a: { title: string } | null) => {
    usePopups.getState().popMessage(m)
    m.resolve(a)
  }
  return (
    <Modal
      title={`${m.server} asks`}
      onClose={() => answer(null)}
      footer={
        <>
          <Button onClick={() => answer(null)}>Dismiss</Button>
          {m.actions.map((a, i) => (
            <Button key={i} variant={i === 0 ? 'primary' : 'default'} onClick={() => answer(a)}>
              {a.title}
            </Button>
          ))}
        </>
      }
    >
      <div className={m.level === 1 ? 'wb-danger' : undefined} style={{ whiteSpace: 'pre-wrap' }}>
        {m.message}
      </div>
    </Modal>
  )
}
