// The `devcontainer` panel: the project's config (picker, summary, risks, engine), its
// container (state, ports and how this computer reaches them), and the actions:
// Start (confirmed), Stop, Rebuild (confirmed), Remove, Open shell in container, and
// whether terminals and runs go into it.

import type { ReactNode } from 'react'
import { Container, Eye, FilePlus2, Hammer, Info, Play, RefreshCw, ShieldAlert, Square, SquareTerminal, Trash2, TriangleAlert } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { confirmDialog, openPanel } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Badge, Button, Checkbox, EmptyState, ErrorBox, IconButton, Loading, Section, Select, Spinner, StatusDot, TimeAgo } from '@/ui'
import { openContainerShell, openTerminalPanel, removeContainer, saveSettings, stopContainer, useDevcontainer } from './api'
import { openStartDialog } from './ConfirmDialog'
import { engineLabel, portText, relPath, riskCounts, sortedRisks, sourceKindLabel, sourceText, stateLabel, stateTone } from './logic'
import { openScaffoldDialog } from './ScaffoldDialog'
import type { DcView, PortView } from './types'

function KV({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="wb-dc-kv">
      <span className="k">{k}</span>
      <span className="v">{children}</span>
    </div>
  )
}

function openPort(pid: string, p: PortView) {
  openPanel({ kind: 'app', id: `app:${pid}:devcontainer-${p.port}`, title: `:${p.port}`, params: { projectId: pid, url: p.url } })
}

async function confirmRemove(pid: string, v: DcView) {
  const ok = await confirmDialog({
    title: 'Remove the dev container?',
    message: `${v.container?.compose ? `docker compose down (project ${v.container.compose})` : `docker rm -f ${v.container?.name}`}.\n\nFiles in the project stay (they are on this computer); anything else inside the container is lost. Images and volumes are kept.`,
    confirmLabel: 'Remove',
    danger: true,
  })
  if (ok) await removeContainer(pid)
}

export function DevcontainerPanel({ params }: PanelProps<{ projectId: string }>) {
  const pid = params.projectId
  const q = useDevcontainer(pid)
  const projects = useProjects()
  const rootAbs = projects.data?.find((p) => p.id === pid)?.rootAbs
  const v = q.data

  if (q.isLoading) return <Loading />
  if (q.error) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  if (!v) return null

  if (!v.configs.length && !v.container) {
    return (
      <EmptyState
        icon={Container}
        title="No devcontainer.json"
        action={
          <Button variant="primary" icon={FilePlus2} onClick={() => openScaffoldDialog(pid)}>
            Create devcontainer.json…
          </Button>
        }
      >
        A dev container runs this project's shells, run configurations and agents in a container built from the repository's
        devcontainer.json, while the files stay here.
      </EmptyState>
    )
  }

  const running = v.state === 'running'
  // The shown container belongs to the selected config (else to another of the project's).
  const own = !!v.container && v.container.configFile === v.config
  const busy = !!v.operation
  const plan = v.plan
  const counts = plan ? riskCounts(plan.risks) : null
  const tone = stateTone(v.state)
  return (
    <div className="wb-fill wb-dc-panel">
      <div className="wb-dc-head">
        <Container size={16} />
        <b className="wb-ellipsis">{plan?.config.name ?? 'Dev container'}</b>
        <span className={`wb-dc-state ${tone}`}>
          {busy ? <Spinner size={10} /> : <StatusDot tone={tone} />}
          {busy ? `${v.operation!.kind}…` : stateLabel(v.state)}
        </span>
        {v.inContainer && <Badge tone="accent">terminals and runs inside</Badge>}
        {v.configs.length > 1 && (
          <Select value={v.config ?? ''} onChange={(e) => void saveSettings(pid, { config: e.target.value })} title="devcontainer.json" aria-label="Configuration">
            {v.configs.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </Select>
        )}
        <span style={{ flex: 1 }} />
        <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => void q.refetch()} />
      </div>
      <div className="wb-dc-actions">
        <Button variant="primary" icon={Play} disabled={busy || !plan || !plan.engine} onClick={() => openStartDialog(pid, false, v.config)}>
          {running && own ? 'Attach…' : 'Start…'}
        </Button>
        <Button icon={Square} disabled={busy || !running} onClick={() => void stopContainer(pid)}>
          Stop
        </Button>
        <Button icon={Hammer} disabled={busy || !plan || !plan.engine} onClick={() => openStartDialog(pid, true, v.config)}>
          Rebuild…
        </Button>
        <Button icon={Trash2} disabled={busy || !v.container} onClick={() => void confirmRemove(pid, v)}>
          Remove…
        </Button>
        <Button icon={SquareTerminal} disabled={!running} onClick={() => void openContainerShell(pid)}>
          Open shell in container
        </Button>
        {v.operation?.terminalId && (
          <Button size="small" icon={Eye} onClick={() => openTerminalPanel({ id: v.operation!.terminalId!, title: 'Dev container' })}>
            Show output
          </Button>
        )}
      </div>
      <div className="wb-scroll">
        {v.error && (
          <div className="wb-error wb-dc-block">
            <TriangleAlert size={14} className="wb-danger" /> {v.error}
          </div>
        )}
        {v.planError && <div className="wb-error wb-dc-block">{v.planError}</div>}
        {plan?.problems.map((p) => (
          <div key={p} className="wb-error setup wb-dc-block">
            {p}
          </div>
        ))}

        <Section title="Container">
          <div className="wb-dc-block">
            {v.container ? (
              <>
                <KV k="Name">
                  <code>{v.container.name}</code> <span className="wb-subtle mono">{v.container.id}</span>
                  {!v.container.managed && <Badge>made outside Workbench</Badge>}
                  {!own && <Badge tone="warning">from {v.container.configFile ?? 'another config'}</Badge>}
                </KV>
                <KV k="Image">
                  <code>{v.container.image}</code>
                </KV>
                <KV k="Status">
                  {v.container.status} · created <TimeAgo time={v.container.created} />
                  {v.container.compose && <span className="wb-muted"> · compose project {v.container.compose}</span>}
                </KV>
                {running && (
                  <KV k="Inside">
                    user <code>{v.remoteUser ?? 'the image default'}</code> in <code>{v.workspaceFolder ?? '/'}</code>
                    {v.container.ip && <span className="wb-muted"> · {v.container.ip}</span>}
                  </KV>
                )}
              </>
            ) : (
              <div className="wb-muted wb-small">No container yet. Start builds and creates it after you review what it runs.</div>
            )}
            <Checkbox checked={v.useContainer} disabled={!v.container} onChange={(on) => void saveSettings(pid, { useContainer: on })}>
              Run terminals and runs in the container
            </Checkbox>
            <div className="wb-xs wb-subtle">
              New shells and run configurations of this project start inside while it runs; a shell or a run can still use the host (New shell ▸ Host shell;
              a run's menu ▸ Always run on the host{v.hostRuns.length ? `: ${v.hostRuns.join(', ')}` : ''}).
            </div>
          </div>
        </Section>

        {running && (
          <Section title="Ports" count={v.ports.length}>
            <div className="wb-dc-block">
              {v.ports.length === 0 && <div className="wb-muted wb-small">No forwarded ports.</div>}
              {v.ports.map((p) => (
                <div key={p.port} className="wb-list-row wb-dc-port" onDoubleClick={() => openPort(pid, p)}>
                  <span className="mono">{portText(p)}</span>
                  {p.via === 'container-ip' && (
                    <span className="wb-xs wb-subtle" title="Not published: this computer reaches it on the container's own address (Linux bridge network)">
                      not published
                    </span>
                  )}
                  <span className="wb-grow" />
                  <IconButton icon={Eye} size="small" label={`Preview ${p.url}`} onClick={() => openPort(pid, p)} />
                </div>
              ))}
            </div>
          </Section>
        )}

        {plan && (
          <Section title="Configuration">
            <div className="wb-dc-block">
              <KV k="File">
                <code>{plan.config.path}</code>
                {v.approved ? <Badge tone="success">approved</Badge> : <Badge tone="warning">not approved yet</Badge>}
              </KV>
              <KV k={sourceKindLabel(plan.config.source)}>
                <code>{sourceText(plan.config.source, rootAbs)}</code>
              </KV>
              <KV k="Engine">{engineLabel(plan)}</KV>
              <KV k="Workspace">
                {plan.config.workspaceMount ? <code>{relPath(rootAbs, plan.config.workspaceMount.source)}</code> : 'compose'} → <code>{plan.config.workspaceFolder}</code>
              </KV>
              {Object.keys(plan.config.features).length > 0 && <KV k="Features">{Object.keys(plan.config.features).join(', ')}</KV>}
              {plan.hooks.length > 0 && <KV k="Lifecycle">{plan.hooks.map((h) => h.key).join(', ')}</KV>}
              {plan.ports.length > 0 && <KV k="Forwards">{plan.ports.join(', ')}</KV>}
            </div>
          </Section>
        )}

        {plan && counts && (
          <Section title="Review" count={plan.risks.length} defaultOpen={counts.danger > 0}>
            <div className="wb-dc-block">
              <div className="wb-row">
                {counts.danger > 0 && <Badge tone="danger">{counts.danger} dangerous</Badge>}
                {counts.warning > 0 && <Badge tone="warning">{counts.warning} to review</Badge>}
                {counts.info > 0 && <Badge>{counts.info} info</Badge>}
              </div>
              <ul className="wb-dc-risks compact">
                {sortedRisks(plan.risks).map((r, i) => (
                  <li key={i} className={r.level}>
                    {r.level === 'danger' ? <ShieldAlert size={13} /> : r.level === 'warning' ? <TriangleAlert size={13} /> : <Info size={13} />}
                    <div>
                      <code>{r.item}</code>
                      <div className="wb-xs wb-muted">{r.message}</div>
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          </Section>
        )}

        {running && (
          <Section title="Agents" defaultOpen={false}>
            <div className="wb-dc-block">
              {Object.entries(v.agents).map(([name, path]) => (
                <KV key={name} k={name}>
                  {path ? <code>{path}</code> : <span className="wb-muted">not installed in the container</span>}
                </KV>
              ))}
              <div className="wb-xs wb-subtle">
                {v.bridge
                  ? `Sessions inside reach Workbench's hooks and MCP at ${v.bridge} (that listener answers nothing else, and only with a session's token).`
                  : 'Starting a session inside opens a hooks/MCP listener on the container network gateway.'}{' '}
                Claude Code inside uses its own login (log in there once, or mount your config in devcontainer.json).
              </div>
            </div>
          </Section>
        )}

        <Section title="Engines" defaultOpen={false}>
          <div className="wb-dc-block">
            <KV k="Docker">{v.engines.docker ?? <span className="wb-danger">{v.engines.dockerError ?? 'not available'}</span>}</KV>
            <KV k="Compose">{v.engines.compose ?? <span className="wb-muted">not installed</span>}</KV>
            <KV k="CLI">{v.engines.cli ? <code>{v.engines.cli}</code> : <span className="wb-muted">not installed (needed for features)</span>}</KV>
            <KV k="Preference">{v.engines.preference}</KV>
          </div>
        </Section>
      </div>
    </div>
  )
}
