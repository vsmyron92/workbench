// Phone tab: session list → full-screen terminal with touch scrolling, an extra-keys bar
// (phones lack Esc/Tab/Ctrl/arrows) and a compose box that sends through /input — the
// robust way to type on a phone (autocorrect, dictation, IME).

import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, ChevronLeft, MoreVertical, Plus, SendHorizontal, SquareTerminal } from 'lucide-react'
import { useTerminals } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { toastError } from '@/shell/actions'
import { Button, Checkbox, EmptyState, ErrorBox, IconButton, Loading, showMenuAt, TimeAgo } from '@/ui'
import { terminalMenu } from './actions'
import { terminalsApi } from './api'
import { newShell } from './commands'
import { pendingOf } from './lib/permission'
import { EXTRA_KEYS } from './lib/protocol'
import { agentMeta, agentSessions, composeBlocked, isRunning, sortSessions } from './lib/sessions'
import { NewSessionForm } from './NewSession'
import { providerLabel } from './lib/providers'
import { ContainerBadge, ProviderBadge, StateChip, StateDot } from './parts'
import { PermissionRequest } from './Permission'
import { dismissPermissionToasts } from './permissionToasts'
import { useAgentsUi } from './store'
import { ExitBanner, useMarkSeen } from './TerminalPanel'
import { TerminalView, type TerminalViewHandle } from './TerminalView'

function Compose({ t, onUse }: { t: TerminalInfo; onUse: () => void }) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const ref = useRef<HTMLTextAreaElement>(null)
  // Never type into an agent's dialog: the Enter would answer it (the server refuses too).
  const blocked = composeBlocked(t)
  const send = async () => {
    if (!text.trim() || busy || blocked) return
    onUse()
    setBusy(true)
    try {
      await terminalsApi.input(t.id, text, true, true)
      setText('')
    } catch (e) {
      toastError(e, 'Not sent')
    } finally {
      setBusy(false)
    }
  }
  // Grow with the text, up to five lines.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 120)}px`
  }, [text])
  return (
    <>
      {blocked && <div className="wb-ag-compose-note">{blocked}</div>}
      <div className="wb-ag-compose">
        <textarea
          ref={ref}
          className="wb-textarea"
          rows={1}
          value={text}
          placeholder={t.kind === 'agent' ? `Message ${providerLabel(t)}…` : 'Command…'}
          onChange={(e) => setText(e.target.value)}
          onFocus={onUse}
          enterKeyHint="send"
        />
        <IconButton icon={SendHorizontal} label="Send" disabled={!text.trim() || busy || !isRunning(t) || !!blocked} onClick={() => void send()} />
      </div>
    </>
  )
}

function MobileTerminal({ t, onBack, onReplaced }: { t: TerminalInfo; onBack: () => void; onReplaced: (next: TerminalInfo) => void }) {
  const view = useRef<TerminalViewHandle | null>(null)
  useMarkSeen(t, true)
  const attention = isRunning(t) ? t.agent?.attention : null
  const pending = pendingOf(t)
  const pendingId = pending?.id
  useEffect(() => {
    if (pendingId) dismissPermissionToasts(t.id)
  }, [pendingId, t.id])
  // Using the keys bar or the compose box is using this view: the PTY takes the phone's size.
  const claim = () => view.current?.claim()
  return (
    <div className="wb-ag-mobile-term">
      <div className="wb-ag-mobile-bar">
        <IconButton icon={ChevronLeft} label="Back to sessions" onClick={onBack} />
        <StateDot t={t} />
        <div className="wb-grow">
          <div className="wb-ellipsis wb-ag-mobile-title">{t.title}</div>
          {t.agent && <div className="wb-ellipsis wb-xs wb-muted">{agentMeta(t.agent).join(' · ')}</div>}
        </div>
        <IconButton icon={MoreVertical} label="More" onClick={(e) => showMenuAt(e.currentTarget, terminalMenu(t, { open: false, onReplaced }))} />
      </div>
      {pending ? (
        <div className="wb-ag-mobile-perm">
          <PermissionRequest t={t} compact />
        </div>
      ) : (
        attention && (
          <div className="wb-ag-attention">
            <AlertTriangle size={13} />
            <span>{attention}</span>
          </div>
        )
      )}
      <TerminalView ref={view} terminalId={t.id} visible mobile />
      {!isRunning(t) && <ExitBanner t={t} onReplaced={onReplaced} />}
      {isRunning(t) && (
        <>
          <div className="wb-ag-keys">
            {EXTRA_KEYS.map((k) => (
              <button
                key={k.label}
                title={k.title}
                onPointerDown={(e) => {
                  // Keep the soft keyboard (and focus) where it is.
                  e.preventDefault()
                  claim()
                  view.current?.send(k.seq)
                }}
              >
                {k.label}
              </button>
            ))}
          </div>
          <Compose t={t} onUse={claim} />
        </>
      )}
    </div>
  )
}

function MobileRow({ t, onOpen }: { t: TerminalInfo; onOpen: () => void }) {
  const a = t.agent
  const pending = pendingOf(t)
  return (
    // A div, not a button: a pending request's Allow / Deny buttons sit inside it.
    <div
      className="wb-ag-mobile-row"
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && e.target === e.currentTarget) onOpen()
      }}
    >
      <div className="wb-row">
        {a ? <StateChip t={t} /> : <StateDot t={t} />}
        {a && <ProviderBadge t={t} compact />}
        <ContainerBadge t={t} compact />
        <span className="wb-ellipsis wb-grow wb-ag-mobile-title">{t.title}</span>
        <span className="wb-xs wb-subtle">
          <TimeAgo time={Math.max(a?.lastEventAt ?? 0, t.lastOutputAt) || t.createdAt} />
        </span>
      </div>
      {pending ? (
        <PermissionRequest t={t} compact />
      ) : (
        a &&
        (a.attention || a.lastMessage) &&
        isRunning(t) && <div className={a.attention ? 'wb-ag-mobile-sub attention' : 'wb-ag-mobile-sub'}>{a.attention ?? a.lastMessage}</div>
      )}
    </div>
  )
}

export function MobileAgents({ projectId }: { projectId: string | null }) {
  const { data, isLoading, error } = useTerminals()
  const selected = useAgentsUi((s) => s.mobileTerminal)
  const setSelected = useAgentsUi((s) => s.setMobileTerminal)
  const all = useAgentsUi((s) => s.allProjects)
  const setAll = useAgentsUi((s) => s.setAllProjects)
  const [composing, setComposing] = useState(false)
  const current = data?.find((t) => t.id === selected)

  if (current) return <MobileTerminal t={current} onBack={() => setSelected(null)} onReplaced={(next) => setSelected(next.id)} />
  if (error) return <ErrorBox error={error} />
  if (isLoading) return <Loading />

  const sessions = sortSessions(agentSessions(data, projectId, all).filter((t) => t.open))
  const shells = (data ?? []).filter((t) => t.kind !== 'agent' && t.open && t.projectId === projectId)
  return (
    <div className="wb-scroll wb-ag-mobile-list">
      <div className="wb-ag-mobile-head">
        <span className="wb-ag-section-title" style={{ margin: 0 }}>
          Sessions
        </span>
        <span style={{ flex: 1 }} />
        <Checkbox checked={all} onChange={setAll}>
          All projects
        </Checkbox>
        <Button size="small" variant="primary" icon={Plus} onClick={() => setComposing(!composing)} disabled={!projectId}>
          New
        </Button>
      </div>
      {composing && (
        <NewSessionForm
          projectId={projectId}
          compact
          autoFocus
          onStarted={(t) => {
            setComposing(false)
            setSelected(t.id)
          }}
        />
      )}
      {sessions.map((t) => (
        <MobileRow key={t.id} t={t} onOpen={() => setSelected(t.id)} />
      ))}
      {!sessions.length && !composing && (
        <EmptyState title="No open sessions">Tap New to start an agent session in this project.</EmptyState>
      )}
      <div className="wb-ag-mobile-head">
        <span className="wb-ag-section-title" style={{ margin: 0 }}>
          Terminals
        </span>
        <span style={{ flex: 1 }} />
        <Button size="small" icon={SquareTerminal} onClick={() => void newShell(projectId).then((t) => t && setSelected(t.id))} disabled={!projectId}>
          Shell
        </Button>
      </div>
      {shells.map((t) => (
        <MobileRow key={t.id} t={t} onOpen={() => setSelected(t.id)} />
      ))}
    </div>
  )
}
