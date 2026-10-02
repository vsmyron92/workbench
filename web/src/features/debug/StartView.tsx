// What the Debug tool window shows before a session: the project's launch
// configurations (explicit and derived) to start, attach, and the debug adapters and
// debug servers (OpenOCD, J-Link…, for embedded targets) on this computer with how to
// install or configure the missing ones.

import { AlertTriangle, BugPlay, CheckCircle2, Plug, XCircle } from 'lucide-react'
import { Button, EmptyState, ErrorBox, Loading, Section } from '@/ui'
import { startDebug } from './actions'
import { useAdapters, useConfigs, useServers } from './api'
import { configIcon } from './icons'
import { configSubtitle, configTitle, groupConfigs } from './logic'
import { openAttachPicker, useDebugPrefs } from './store'

/** One program on this computer a debugger needs: found or not, and what to do about it. */
function ToolRow({ label, detail, command, availability, installHint }: { label: string; detail: string; command: string; availability: { available: boolean; path?: string; version?: string; problem?: string }; installHint: string }) {
  return (
    <div className="wb-dbg-adapter" title={availability.path ?? command}>
      {availability.available ? <CheckCircle2 size={14} className="wb-success" /> : <XCircle size={14} className="wb-subtle" />}
      <span className="label">{label}</span>
      <span className="wb-muted wb-small wb-ellipsis">{detail}</span>
      <span className="wb-grow" />
      {availability.available ? (
        <span className="wb-muted wb-small wb-ellipsis">{availability.version ?? availability.path}</span>
      ) : (
        <span className="wb-small wb-ellipsis hint" title={installHint}>
          {availability.problem}. {installHint}
        </span>
      )}
    </div>
  )
}

function Adapters({ projectId }: { projectId: string }) {
  const q = useAdapters(projectId)
  if (q.isLoading) return <Loading label="Looking for debuggers…" />
  if (q.isError) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  return (
    <div className="wb-dbg-adapters">
      {q.data!.adapters.map((a) => (
        <ToolRow key={a.id} label={a.label} detail={a.languages.join(', ')} command={a.command} availability={a.availability} installHint={a.installHint} />
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

/** The programs that sit between gdb and a microcontroller (a debug probe's server) or
 *  stand in for one (QEMU): a configuration's `[debug.remote]` names one. */
function Servers({ projectId }: { projectId: string }) {
  const q = useServers(projectId)
  if (q.isLoading) return <Loading label="Looking for debug servers…" />
  if (q.isError) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  return (
    <div className="wb-dbg-adapters">
      {q.data!.servers.map((s) => (
        <ToolRow key={s.id} label={s.label} detail={s.id} command={s.command} availability={s.availability} installHint={s.installHint} />
      ))}
      {q.data!.warnings.map((w) => (
        <div key={w} className="wb-small wb-warning">
          {w}
        </div>
      ))}
      <div className="wb-xs wb-muted wb-dbg-adapters-foot">
        A configuration with <code>[debug.remote]</code> starts one of these, connects gdb to it and downloads the program. Override a preset or add one under <code>[debug.servers.&lt;id&gt;]</code> in config.toml.
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
          Add a <code>[[debug]]</code> entry to .workbench.toml (name, program, args, pre_launch…), with a <code>[debug.remote]</code> table for a microcontroller (server = &quot;openocd&quot;, &quot;jlink&quot;…). Cargo targets, CMake executables, Python run configurations and Go main packages show up here by themselves.
        </EmptyState>
      ) : (
        groups.map((g) => (
          <Section key={g.title} title={g.title} count={g.items.length} defaultOpen={g.items.length <= 30}>
            {g.items.map((c) => {
              const I = configIcon(c)
              return (
                <div
                  key={c.name}
                  className={`wb-list-row wb-dbg-config${c.name === last ? ' selected' : ''}`}
                  onDoubleClick={() => void startDebug(projectId, c.name)}
                  onClick={() => useDebugPrefs.getState().setConfig(projectId, c.name)}
                  title={configTitle(c)}
                >
                  <I size={14} className="wb-muted" />
                  <span className="wb-ellipsis name">{c.name}</span>
                  <span className="wb-grow wb-ellipsis wb-muted wb-small">{configSubtitle(c)}</span>
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
      <Section title="Debug servers (embedded targets)" defaultOpen={(q.data?.configs ?? []).some((c) => c.remote && !c.remote.serverAvailable)}>
        <Servers projectId={projectId} />
      </Section>
    </div>
  )
}
