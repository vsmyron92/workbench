// What the Debug tool window shows before a session: the project's launch
// configurations (explicit and derived) to start, attach, and the debug adapters on
// this computer with how to install or configure the missing ones.

import { AlertTriangle, BugPlay, CheckCircle2, Plug, XCircle } from 'lucide-react'
import { Button, EmptyState, ErrorBox, Loading, Section } from '@/ui'
import { startDebug } from './actions'
import { useAdapters, useConfigs } from './api'
import { ORIGIN_ICON } from './icons'
import { groupConfigs } from './logic'
import { openAttachPicker, useDebugPrefs } from './store'

function Adapters({ projectId }: { projectId: string }) {
  const q = useAdapters(projectId)
  if (q.isLoading) return <Loading label="Looking for debuggers…" />
  if (q.isError) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  return (
    <div className="wb-dbg-adapters">
      {q.data!.adapters.map((a) => (
        <div key={a.id} className="wb-dbg-adapter" title={a.availability.path ?? a.command}>
          {a.availability.available ? <CheckCircle2 size={14} className="wb-success" /> : <XCircle size={14} className="wb-subtle" />}
          <span className="label">{a.label}</span>
          <span className="wb-muted wb-small wb-ellipsis">{a.languages.join(', ')}</span>
          <span className="wb-grow" />
          {a.availability.available ? (
            <span className="wb-muted wb-small wb-ellipsis">{a.availability.version ?? a.availability.path}</span>
          ) : (
            <span className="wb-small wb-ellipsis hint" title={a.installHint}>
              {a.availability.problem}. {a.installHint}
            </span>
          )}
        </div>
      ))}
      {q.data!.warnings.map((w) => (
        <div key={w} className="wb-small wb-warning">
          {w}
        </div>
      ))}
      <div className="wb-xs wb-muted wb-dbg-adapters-foot">
        Adapters come from config.toml: override a preset or add one under <code>[debug.adapters.&lt;id&gt;]</code>; pick one per language with <code>[debug] default_adapter</code>.
      </div>
    </div>
  )
}

export function StartView({ projectId }: { projectId: string }) {
  const q = useConfigs(projectId)
  const picked = useDebugPrefs((s) => s.config[projectId])
  if (q.isLoading) return <Loading label="Reading launch configurations…" />
  if (q.isError) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  const groups = groupConfigs(q.data?.configs ?? [])
  const last = picked ?? q.data?.lastConfig
  return (
    <div className="wb-scroll wb-dbg-start">
      {groups.length === 0 ? (
        <EmptyState icon={BugPlay} title="No launch configurations">
          Add a <code>[[debug]]</code> entry to .workbench.toml (name, program, args, pre_launch…). Cargo targets, CMake executables, Python run configurations and Go main packages show up here by themselves.
        </EmptyState>
      ) : (
        groups.map((g) => (
          <Section key={g.title} title={g.title} count={g.items.length} defaultOpen={g.items.length <= 30}>
            {g.items.map((c) => {
              const I = ORIGIN_ICON[c.origin]
              return (
                <div
                  key={c.name}
                  className={`wb-list-row wb-dbg-config${c.name === last ? ' selected' : ''}`}
                  onDoubleClick={() => void startDebug(projectId, c.name)}
                  onClick={() => useDebugPrefs.getState().setConfig(projectId, c.name)}
                  title={[c.program ?? c.module ?? '', c.preLaunch ? `before: ${c.preLaunch}` : '', ...c.problems.map((p) => `⚠ ${p}`)].filter(Boolean).join('\n')}
                >
                  <I size={14} className="wb-muted" />
                  <span className="wb-ellipsis name">{c.name}</span>
                  <span className="wb-grow wb-ellipsis wb-muted wb-small">{c.preLaunch ? `${c.adapterLabel ?? ''} · ${c.preLaunch}` : (c.adapterLabel ?? '')}</span>
                  {c.problems.length > 0 && <AlertTriangle size={13} className="wb-warning" aria-label={c.problems.join('; ')} />}
                  <Button size="small" variant="ghost" icon={BugPlay} className="go" onClick={(e) => {
                      e.stopPropagation()
                      void startDebug(projectId, c.name)
                    }} aria-label={`Debug ${c.name}`}>
                    Debug
                  </Button>
                </div>
              )
            })}
          </Section>
        ))
      )}
      <div className="wb-dbg-start-actions">
        <Button size="small" icon={Plug} onClick={() => openAttachPicker(projectId)}>
          Attach to Process…
        </Button>
      </div>
      <Section title="Debug adapters" defaultOpen={groups.length === 0}>
        <Adapters projectId={projectId} />
      </Section>
    </div>
  )
}
