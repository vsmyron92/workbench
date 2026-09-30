// Invisible providers: keep the ['terminals'] cache fresh from terminal.* events, turn
// agent.attention into notifications, and host the feature's dialogs.

import { useEffect, useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Radio } from 'lucide-react'
import { subscribe } from '@/api/events'
import { qk } from '@/api/queries'
import type { AgentState, PendingPermission, TerminalInfo } from '@/api/types'
import { toast, toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Button, Field, Input, Modal, Select } from '@/ui'
import { openTerminal, terminalsApi } from './api'
import { HistoryList } from './AgentsHome'
import { glanceText, oneTapAllow, summaryParts, summaryShowsAll } from './lib/permission'
import { removeTerminal, upsertTerminal } from './lib/sessions'
import { NewSessionForm } from './NewSession'
import { answerPermission } from './Permission'
import { hasPermissionToast, notePermissionToast, settlePermissionToasts } from './permissionToasts'
import { setQueryClient } from './queryAccess'
import { isTerminalVisible, useAgentsUi } from './store'

export function TerminalsSync({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  useEffect(() => {
    setQueryClient(qc)
    return () => setQueryClient(null)
  }, [qc])
  useEffect(() => {
    const upsert = (t: TerminalInfo) => qc.setQueryData<TerminalInfo[]>(qk.terminals, (old) => (old ? upsertTerminal(old, t) : old))
    const history = (t: { kind?: string }) => {
      if (!t.kind || t.kind === 'agent') void qc.invalidateQueries({ queryKey: ['agents', 'history'] })
    }
    const offs = [
      subscribe('terminal.created', (ev) => {
        upsert(ev.data as TerminalInfo)
        history(ev.data as TerminalInfo)
      }),
      subscribe('terminal.updated', (ev) => upsert(ev.data as TerminalInfo)),
      subscribe('terminal.exited', (ev) => {
        upsert(ev.data as TerminalInfo)
        history(ev.data as TerminalInfo)
      }),
      subscribe('terminal.removed', (ev) => {
        const { id } = ev.data as { id: string }
        qc.setQueryData<TerminalInfo[]>(qk.terminals, (old) => (old ? removeTerminal(old, id) : old))
        history({})
      }),
      subscribe('resync', () => void qc.invalidateQueries({ queryKey: qk.terminals })),
      // Providers are configured in config.toml: a saved config may add, remove or fix one.
      subscribe('settings.changed', () => void qc.invalidateQueries({ queryKey: ['agents', 'defaults'] })),
    ]
    return () => offs.forEach((o) => o())
  }, [qc])
  return <>{children}</>
}

interface Attention {
  terminalId: string
  state: AgentState
  message: string
  title: string
  /** A permission request Workbench can answer (Claude Code). */
  permission?: PendingPermission | null
}

/**
 * A toast when a session wants attention and is not on screen. OS notifications belong to
 * the platform slice alone: the server's desktop notifications on the Workbench computer,
 * browser notifications on remote devices while the tab is hidden. A second one here
 * would double every notification.
 *
 * A permission request's toast has Allow / Deny and stays until it is answered, or
 * settled elsewhere (the terminal, a phone, a timeout): then it goes away by itself.
 */
export function AttentionNotifier({ children }: { children?: ReactNode }) {
  useEffect(() => {
    const offAttention = subscribe('agent.attention', (ev) => {
      const a = ev.data as Attention
      const open = () => openTerminal({ id: a.terminalId, title: a.title })
      if (document.visibilityState !== 'visible') return
      if (isTerminalVisible(a.terminalId)) return
      const p = a.permission
      if (p && a.state === 'needs_permission' && !hasPermissionToast(p.id)) {
        const { lead, code, tail } = summaryParts(p.summary)
        const allow = { label: 'Allow', variant: 'primary' as const, run: () => void answerPermission(a.terminalId, p, { decision: 'allow' }) }
        const deny = { label: 'Deny', run: () => void answerPermission(a.terminalId, p, { decision: 'deny' }) }
        // One tap allows only what the toast shows whole: otherwise read it first.
        const oneTap = oneTapAllow(p)
        const toastId = toast('warning', `${a.title} asks for permission`, {
          detail: !oneTap
            ? `${[lead, code, tail].filter(Boolean).join(' ')} — open it to read the whole request before allowing.`
            : code !== null
              ? lead
              : p.summary,
          code: oneTap && (code !== null || !summaryShowsAll(p)) ? glanceText(p) : undefined,
          timeout: 0,
          actions: oneTap ? [allow, deny, { label: 'Open', run: open }] : [deny, { label: 'Open to review', run: open }],
        })
        notePermissionToast(p.id, toastId, a.terminalId, p.since)
        return
      }
      if (p) return
      if (a.state === 'idle') toast('success', `${a.title} finished`, { detail: a.message, action: { label: 'Open', run: open } })
      else toast(a.state === 'error' ? 'error' : 'warning', `${a.title}: ${a.message}`, { action: { label: 'Open', run: open }, timeout: 9000 })
    })
    const offUpdated = subscribe('terminal.updated', (ev) => settlePermissionToasts(ev.data as TerminalInfo))
    const offExited = subscribe('terminal.exited', (ev) => settlePermissionToasts(ev.data as TerminalInfo))
    return () => {
      offAttention()
      offUpdated()
      offExited()
    }
  }, [])
  return <>{children}</>
}

function RemoteControlDialog({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const [spawn, setSpawn] = useState('same-dir')
  const [name, setName] = useState('')
  const [mode, setMode] = useState('')
  const [busy, setBusy] = useState(false)
  const start = async () => {
    setBusy(true)
    try {
      const t = await terminalsApi.remoteControl({ projectId, spawn, name: name.trim() || undefined, permissionMode: mode || undefined })
      openTerminal(t)
      toast('success', 'Remote Control server started', { detail: 'Its claude.ai link appears on the Agents home once it is up.' })
      onClose()
    } catch (e) {
      toastError(e, 'Could not start Remote Control')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Start a Remote Control server"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={Radio} loading={busy} onClick={() => void start()}>
            Start
          </Button>
        </>
      }
    >
      <div className="wb-muted wb-small">
        Runs <code>claude remote-control</code> in this project so you can start and drive sessions from claude.ai or the Claude app.
      </div>
      <Field label="Sessions">
        <Select value={spawn} onChange={(e) => setSpawn(e.target.value)}>
          <option value="same-dir">Share this directory</option>
          <option value="worktree">A git worktree per session</option>
          <option value="session">One session, then stop</option>
        </Select>
      </Field>
      <Field label="Name (optional)">
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Shown on claude.ai" />
      </Field>
      <Field label="Permission mode">
        <Select value={mode} onChange={(e) => setMode(e.target.value)}>
          <option value="">Default</option>
          {['acceptEdits', 'auto', 'plan', 'manual', 'dontAsk', 'bypassPermissions'].map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </Select>
      </Field>
    </Modal>
  )
}

export function AgentDialogs({ children }: { children?: ReactNode }) {
  const dialog = useAgentsUi((s) => s.dialog)
  const close = () => useAgentsUi.getState().openDialog(null)
  const projectId = useUi((s) => s.projectId)
  let node: ReactNode = null
  if (dialog?.kind === 'new') {
    node = (
      <Modal title="New agent session" onClose={close} wide>
        <NewSessionForm projectId={projectId} prefill={dialog.prefill} autoFocus onStarted={close} />
      </Modal>
    )
  } else if (dialog?.kind === 'resume' && projectId) {
    node = (
      <Modal title="Resume a session" onClose={close} wide>
        <HistoryList projectId={projectId} onDone={close} limit={40} />
      </Modal>
    )
  } else if (dialog?.kind === 'remote' && projectId) {
    node = <RemoteControlDialog projectId={projectId} onClose={close} />
  }
  return (
    <>
      {children}
      {node}
    </>
  )
}
