// "Deploy to <env>…": shows the gates the server enforces (branch, pipeline, order),
// the exact command and target, then asks for the env's confirmation (typed for
// production). The server re-checks everything on the actual deploy call.

import { useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { create } from 'zustand'
import { AlertTriangle, CheckCircle2, CircleMinus, ExternalLink, RefreshCw, Rocket, XCircle } from 'lucide-react'
import { confirmDialog } from '@/shell/actions'
import { Badge, Button, ErrorBox, Input, Loading, Modal, Spinner } from '@/ui'
import { deploy, deployCheck, openExternal, openTerminal } from './api'
import type { Gate } from './types'

const useDeployDialog = create<{ target: { pid: string; env: string } | null; set: (t: { pid: string; env: string } | null) => void }>()(
  (set) => ({ target: null, set: (target) => set({ target }) }),
)

export function openDeployDialog(pid: string, env: string) {
  useDeployDialog.getState().set({ pid, env })
}

/** Mounted once (provider): renders the app, plus the dialog when one is open. */
export function DeployDialogHost({ children }: { children?: ReactNode }) {
  const { target, set } = useDeployDialog()
  return (
    <>
      {children}
      {target && <DeployDialog key={`${target.pid}:${target.env}`} pid={target.pid} env={target.env} onClose={() => set(null)} />}
    </>
  )
}

const GATE_ICON = { pass: CheckCircle2, fail: XCircle, warn: AlertTriangle, skip: CircleMinus }
const GATE_CLASS = { pass: 'wb-success', fail: 'wb-danger', warn: 'wb-warning', skip: 'wb-subtle' }

function GateRow({ g }: { g: Gate }) {
  const I = GATE_ICON[g.status]
  return (
    <div className="wb-apps-gate">
      <I size={15} className={GATE_CLASS[g.status]} />
      <div className="wb-grow">
        <div className="label">{g.label}</div>
        <div className="wb-small wb-muted">{g.detail}</div>
      </div>
      {g.url && (
        <button className="wb-icon-btn small" title="Open pipeline" aria-label="Open pipeline" onClick={() => openExternal(g.url!)}>
          <ExternalLink size={13} />
        </button>
      )}
    </div>
  )
}

function DeployDialog({ pid, env, onClose }: { pid: string; env: string; onClose: () => void }) {
  const [shaInput, setShaInput] = useState('')
  const [sha, setSha] = useState('')
  const [busy, setBusy] = useState(false)
  const [hidden, setHidden] = useState(false)
  const plan = useQuery({
    queryKey: ['apps', 'deploy-plan', pid, env, sha],
    queryFn: () => deployCheck(pid, env, sha || undefined),
    retry: false,
    staleTime: 0,
    gcTime: 0,
  })
  const p = plan.data

  const run = async () => {
    if (!p) return
    let confirmation: string | boolean = true
    if (p.confirm !== 'none') {
      setHidden(true)
      const ok = await confirmDialog({
        title: `Deploy ${p.sha8} to ${env}?`,
        message: `${p.target === 'local' ? 'Runs locally' : `Runs on ${p.target}`}:\n${p.command}`,
        confirmLabel: 'Deploy',
        danger: p.kind === 'production',
        typed: p.confirm === 'typed' ? env : undefined,
      })
      setHidden(false)
      if (!ok) return
      confirmation = p.confirm === 'typed' ? env : true
    }
    setBusy(true)
    const t = await deploy(pid, env, p.sha, confirmation)
    setBusy(false)
    if (t) onClose()
  }

  if (hidden) return null
  const blocked = p && !p.ok
  return (
    <Modal
      title={
        <span className="wb-row">
          <Rocket size={16} /> Deploy to {env}
          {p && <Badge tone={p.kind === 'production' ? 'danger' : p.kind === 'staging' ? 'warning' : 'accent'}>{p.kind}</Badge>}
        </span>
      }
      onClose={onClose}
      footer={
        <>
          <Button icon={RefreshCw} onClick={() => plan.refetch()} disabled={plan.isFetching}>
            Re-check
          </Button>
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant={p?.kind === 'production' ? 'danger' : 'primary'}
            icon={Rocket}
            loading={busy}
            disabled={!p || !!blocked || !!p.deploying || plan.isFetching}
            onClick={run}
          >
            Deploy {p?.sha8 ?? ''}…
          </Button>
        </>
      }
    >
      <div className="wb-apps-deploy">
        <div className="wb-row">
          <label className="wb-small wb-muted" style={{ width: 64 }}>
            Commit
          </label>
          <Input
            small
            className="mono wb-grow"
            placeholder="HEAD"
            value={shaInput}
            onChange={(e) => setShaInput(e.target.value.trim())}
            onKeyDown={(e) => e.key === 'Enter' && setSha(shaInput)}
            onBlur={() => setSha(shaInput)}
            aria-label="Commit sha (empty = HEAD)"
          />
        </div>
        {plan.isLoading && <Loading label="Checking gates…" />}
        {plan.error && <ErrorBox error={plan.error} onRetry={() => plan.refetch()} />}
        {p && (
          <>
            <div className="wb-apps-deploy-commit">
              <span className="mono">{p.sha8}</span>
              <span className="wb-ellipsis wb-grow" title={p.subject}>
                {p.subject ?? ''}
              </span>
              {p.branch && <Badge>{p.branch}</Badge>}
            </div>
            <div className="wb-apps-gates">
              {plan.isFetching && (
                <div className="wb-row wb-small wb-muted">
                  <Spinner size={12} /> re-checking…
                </div>
              )}
              {p.gates.map((g) => (
                <GateRow key={g.id} g={g} />
              ))}
            </div>
            <div className="wb-apps-deploy-cmd">
              <div className="wb-xs wb-muted">{p.target === 'local' ? 'Runs locally' : `Runs on ${p.target} over ssh`}</div>
              <code>{p.command}</code>
            </div>
            {p.deploying && (
              <div className="wb-row wb-small wb-warning">
                <Spinner size={12} /> A deploy to {env} is running.
                <Button size="small" onClick={() => openTerminal(p.deploying!, `Deploy → ${env}`)}>
                  Show
                </Button>
              </div>
            )}
            {blocked && <div className="wb-small wb-danger">Deploy is blocked until every gate passes.</div>}
          </>
        )}
      </div>
    </Modal>
  )
}
