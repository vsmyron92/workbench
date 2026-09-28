// "Start dev container?": everything the start will run, from the repository's own
// devcontainer.json (engine, image or Dockerfile, runArgs, mounts, ports, lifecycle
// commands, features), dangerous items first and prominent. The server only starts
// with the hash of exactly this plan; a change in between shows the dialog again.

import { useEffect, useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { create } from 'zustand'
import { Container, Hammer, Info, Play, ShieldAlert, TriangleAlert } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { Badge, Button, Checkbox, DialogBoundary, ErrorBox, Loading, Modal } from '@/ui'
import { fetchView, startWithApproval } from './api'
import { engineLabel, relPath, riskCounts, sortedRisks, sourceKindLabel, sourceText } from './logic'
import type { DevConfig, Plan, Risk } from './types'

interface Target {
  pid: string
  rebuild: boolean
  config: string | null
  /** The plan changed between the dialog and the click. */
  stale?: boolean
}

const useStartDialog = create<{ target: Target | null; set: (t: Target | null) => void }>()((set) => ({
  target: null,
  set: (target) => set({ target }),
}))

/** Ask to start (or rebuild) the project's dev container. */
export function openStartDialog(pid: string, rebuild = false, config: string | null = null) {
  useStartDialog.getState().set({ pid, rebuild, config })
}

export function StartDialogHost() {
  const { target, set } = useStartDialog()
  if (!target) return null
  const key = `${target.pid}:${target.rebuild}:${target.config}:${target.stale}`
  return (
    <DialogBoundary key={key} title="Dev container" onClose={() => set(null)}>
      <StartDialog target={target} onClose={() => set(null)} onStale={() => set({ ...target, stale: true })} />
    </DialogBoundary>
  )
}

function Row({ label, children, tone }: { label: string; children: ReactNode; tone?: 'danger' | 'warning' }) {
  return (
    <>
      <div className="wb-dc-plan-key">{label}</div>
      <div className={`wb-dc-plan-val${tone ? ` ${tone}` : ''}`}>{children}</div>
    </>
  )
}

/** Items the risks name, for highlighting them where they are listed. */
function flagged(risks: Risk[], level: 'danger' | 'warning') {
  return (text: string) => risks.some((r) => r.level === level && r.item.includes(text))
}

function PlanTable({ plan, rootAbs }: { plan: Plan; rootAbs?: string }) {
  const c: DevConfig = plan.config
  const danger = flagged(plan.risks, 'danger')
  const s = c.source
  const args = s.kind === 'dockerfile' ? Object.entries(s.args) : []
  const features = Object.keys(c.features)
  const ports = [...c.forwardPorts, ...c.appPorts]
  return (
    <div className="wb-dc-plan">
      <Row label="Engine">{engineLabel(plan)}</Row>
      <Row label={sourceKindLabel(s)}>
        <code>{sourceText(s, rootAbs)}</code>
        {s.kind === 'compose' && s.runServices.length > 0 && <span className="wb-muted"> · also starts {s.runServices.filter((x) => x !== s.service).join(', ') || '—'}</span>}
      </Row>
      {args.length > 0 && (
        <Row label="Build args">
          {args.map(([k, v]) => (
            <code key={k} className="wb-dc-tok">
              {k}={v}
            </code>
          ))}
        </Row>
      )}
      {s.kind === 'dockerfile' && s.options.length > 0 && (
        <Row label="Build options" tone="warning">
          <code>{s.options.join(' ')}</code>
        </Row>
      )}
      <Row label="Workspace" tone={c.workspaceMount && danger(c.workspaceMount.spec) ? 'danger' : undefined}>
        {c.workspaceMount ? (
          <>
            <code>{relPath(rootAbs, c.workspaceMount.source)}</code> → <code>{c.workspaceMount.target}</code>
          </>
        ) : (
          <span className="wb-muted">mounted by the compose file</span>
        )}
        <span className="wb-muted"> · opens in </span>
        <code>{c.workspaceFolder}</code>
      </Row>
      {c.mounts.length > 0 && (
        <Row label="Mounts">
          {c.mounts.map((m) => (
            <div key={m.spec} className={danger(m.spec) ? 'wb-dc-bad' : undefined}>
              <code>{m.spec}</code>
            </div>
          ))}
        </Row>
      )}
      {c.runArgs.length > 0 && (
        <Row label="runArgs">
          <code className={plan.risks.some((r) => r.level === 'danger' && c.runArgs.some((a) => r.item.includes(a))) ? 'wb-dc-bad' : undefined}>{c.runArgs.join(' ')}</code>
        </Row>
      )}
      {(c.privileged || c.capAdd.length > 0 || c.securityOpt.length > 0 || c.init) && (
        <Row label="Privileges" tone={c.privileged || danger('capAdd') || danger('securityOpt') ? 'danger' : undefined}>
          {[c.privileged && 'privileged', ...c.capAdd.map((x) => `cap-add ${x}`), ...c.securityOpt.map((x) => `security-opt ${x}`), c.init && 'init'].filter(Boolean).join(' · ')}
        </Row>
      )}
      {ports.length > 0 && (
        <Row label="Ports">
          {ports.map((p) => (
            <code key={`${p.host ?? ''}${p.port}`} className="wb-dc-tok" title={p.label}>
              {p.host ? `${p.host}:` : ''}
              {p.port}
            </code>
          ))}
          <span className="wb-muted">
            {plan.engine === 'docker' && s.kind !== 'compose' && plan.ports.length ? ' published on 127.0.0.1 only' : ' reached on the container address'}
          </span>
        </Row>
      )}
      {(c.remoteUser || c.containerUser) && (
        <Row label="User">
          {c.remoteUser && (
            <>
              remote <code>{c.remoteUser}</code>
            </>
          )}
          {c.containerUser && (
            <>
              {' '}
              container <code>{c.containerUser}</code>
            </>
          )}
          {c.updateRemoteUserUid && <span className="wb-muted"> · UID matched to yours</span>}
        </Row>
      )}
      {(Object.keys(c.containerEnv).length > 0 || Object.keys(c.remoteEnv).length > 0) && (
        <Row label="Environment">
          {Object.entries(c.containerEnv).map(([k, v]) => (
            <code key={`c${k}`} className="wb-dc-tok" title={v}>
              {k}
            </code>
          ))}
          {Object.keys(c.remoteEnv).map((k) => (
            <code key={`r${k}`} className="wb-dc-tok" title="remoteEnv">
              {k}
            </code>
          ))}
          {c.localEnv.length > 0 && <div className="wb-warning wb-xs">reads {c.localEnv.join(', ')} from this computer's environment</div>}
        </Row>
      )}
      {features.length > 0 && (
        <Row label="Features">
          {features.map((f) => (
            <div key={f}>
              <code>{f}</code>
            </div>
          ))}
        </Row>
      )}
      {plan.hooks.map((h) => (
        <Row key={h.key} label={h.key} tone={h.key === 'initializeCommand' ? 'danger' : undefined}>
          <div className="wb-xs wb-muted">{h.when}</div>
          {h.commands.map((x, i) => (
            <pre key={i} className="wb-dc-cmd">
              {x}
            </pre>
          ))}
        </Row>
      ))}
    </div>
  )
}

function RiskList({ risks, level }: { risks: Risk[]; level: 'danger' | 'warning' | 'info' }) {
  const list = sortedRisks(risks).filter((r) => r.level === level)
  if (!list.length) return null
  const Icon = level === 'danger' ? ShieldAlert : level === 'warning' ? TriangleAlert : Info
  return (
    <ul className={`wb-dc-risks ${level}`}>
      {list.map((r, i) => (
        <li key={i}>
          <Icon size={13} />
          <div>
            <code>{r.item}</code>
            <div className="wb-xs">{r.message}</div>
          </div>
        </li>
      ))}
    </ul>
  )
}

function StartDialog({ target, onClose, onStale }: { target: Target; onClose: () => void; onStale: () => void }) {
  const { pid, rebuild } = target
  const projects = useProjects()
  const rootAbs = projects.data?.find((p) => p.id === pid)?.rootAbs
  // Always the current plan: never a cached one.
  const view = useQuery({ queryKey: ['devcontainer', 'plan', pid, target.config, target.stale], queryFn: () => fetchView(pid, target.config), staleTime: 0, gcTime: 0, retry: false })
  const [ack, setAck] = useState(false)
  const [busy, setBusy] = useState(false)
  const plan = view.data?.plan ?? null
  const counts = plan ? riskCounts(plan.risks) : { danger: 0, warning: 0, info: 0 }
  useEffect(() => setAck(false), [plan?.hash])
  const blocked = !plan || plan.problems.length > 0 || !plan.engine
  const needsAck = counts.danger > 0

  const go = async () => {
    if (!plan) return
    setBusy(true)
    const r = await startWithApproval(pid, view.data?.config ?? null, plan.hash, rebuild)
    setBusy(false)
    if (r.ok) onClose()
    else if (r.stale) onStale()
  }

  // The project's container may belong to another of its configs: then this one gets its own.
  const own = !!view.data?.container && view.data.container.configFile === view.data.config
  const verb = rebuild ? 'Rebuild' : own && view.data?.state === 'running' ? 'Attach to' : 'Start'
  return (
    <Modal
      wide
      title={
        <span className="wb-row">
          <Container size={16} /> {verb} dev container{plan?.config.name ? ` · ${plan.config.name}` : ''}
          {counts.danger > 0 && <Badge tone="danger">{counts.danger} dangerous</Badge>}
          {view.data?.approved && !target.stale && <Badge tone="success">approved before</Badge>}
        </span>
      }
      onClose={onClose}
      footer={
        <>
          {plan && (
            <span className="wb-xs wb-subtle wb-dc-hash" title={`sha256 ${plan.hash}\ncovers ${plan.files.join(', ')}`}>
              approval sha256 {plan.hash.slice(0, 12)}…
            </span>
          )}
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant={needsAck ? 'danger' : 'primary'} icon={rebuild ? Hammer : Play} loading={busy} disabled={blocked || (needsAck && !ack)} onClick={() => void go()}>
            {rebuild ? 'Rebuild' : verb === 'Attach to' ? 'Attach' : 'Start'}
            {needsAck ? ' anyway' : ''}
          </Button>
        </>
      }
    >
      {view.isLoading && <Loading label="Reading devcontainer.json…" />}
      {view.error && <ErrorBox error={view.error} onRetry={() => void view.refetch()} />}
      {view.data?.planError && <div className="wb-error">{view.data.planError}</div>}
      {plan && (
        <div className="wb-dc-confirm">
          {target.stale && (
            <div className="wb-error setup">
              <TriangleAlert size={14} /> The config (or a file it uses) changed after this dialog opened. Review it again.
            </div>
          )}
          {counts.danger > 0 && (
            <div className="wb-dc-danger">
              <div className="wb-dc-danger-title">
                <ShieldAlert size={16} /> This container gets access to your computer
              </div>
              <RiskList risks={plan.risks} level="danger" />
            </div>
          )}
          <div className="wb-small wb-muted">
            {rebuild ? 'The container is removed and created again from ' : 'Workbench will run what '}
            <code>{plan.config.path}</code>
            {rebuild ? '' : ' defines'} on this computer's Docker. It comes from the repository: review it as you would a script.{' '}
            {view.data?.container && !rebuild && own && `The existing container ${view.data.container.name} is reused.`}
            {view.data?.container && !own && `It gets its own container; ${view.data.container.name} (${view.data.container.configFile ?? 'another config'}) is left as it is.`}
          </div>
          {plan.problems.map((p) => (
            <div key={p} className="wb-error setup">
              {p}
            </div>
          ))}
          <PlanTable plan={plan} rootAbs={rootAbs} />
          <RiskList risks={plan.risks} level="warning" />
          <RiskList risks={plan.risks} level="info" />
          {plan.config.notes.length > 0 && <div className="wb-xs wb-subtle">{plan.config.notes.join(' · ')}</div>}
          <div className="wb-xs wb-subtle">
            Workbench remembers this approval for {plan.files.join(', ')}; any change to them asks again. Agents can never start a dev container.
          </div>
          {needsAck && (
            <Checkbox checked={ack} onChange={setAck}>
              I reviewed the {counts.danger} dangerous item{counts.danger > 1 ? 's' : ''} above
            </Checkbox>
          )}
        </div>
      )}
    </Modal>
  )
}
