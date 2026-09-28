import { useMemo, useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { RotateCcw, Save } from 'lucide-react'
import { ApiError, api } from '@/api/client'
import { qk, useProjects } from '@/api/queries'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Button, EmptyState, ErrorBox, Loading, Select, Tabs } from '@/ui'
import { patchSettings, pk, reportApply, useProjectSettings, useSettings } from '../api'
import { DiagnosticLine, Group, Note, Page, Row, StringList, TomlEditor, useDraft, useTomlDiagnostics } from '../common'
import type { GlobalConfig, LayerSaveResult } from '../types'

interface Discovery {
  roots: string[]
  include: string[]
  exclude: string[]
  extraRoots: string[]
}

function DiscoveryGroup() {
  const settings = useSettings()
  const cfg: GlobalConfig | undefined = settings.data?.config
  const saved = useMemo<Discovery | undefined>(
    () => (cfg ? { roots: cfg.projects.roots, include: cfg.projects.include, exclude: cfg.projects.exclude, extraRoots: cfg.extra_roots } : undefined),
    [cfg],
  )
  const { draft, setDraft, dirty, reset } = useDraft(saved)
  const [busy, setBusy] = useState(false)
  if (settings.error) return <ErrorBox error={settings.error} onRetry={() => void settings.refetch()} />
  if (!draft) return <Loading />
  const set = (p: Partial<Discovery>) => setDraft({ ...draft, ...p })
  const save = async () => {
    setBusy(true)
    try {
      const r = await patchSettings(
        { projects: { roots: draft.roots, include: draft.include, exclude: draft.exclude }, extra_roots: draft.extraRoots },
        settings.data?.hash,
      )
      reportApply(r, `Saved — ${r.projects} project${r.projects === 1 ? '' : 's'}`)
    } catch (e) {
      toastError(e, 'Could not save')
    } finally {
      setBusy(false)
    }
  }
  const pathError = (p: string) => (p.startsWith('/') || p.startsWith('~') ? null : 'Use an absolute path or ~/…')
  return (
    <Group
      title="Discovery"
      description="Every git repository directly under a root becomes a project. Include adds single directories anywhere; exclude hides some."
      actions={
        <>
          <Button size="small" icon={RotateCcw} disabled={!dirty || busy} onClick={reset}>
            Revert
          </Button>
          <Button size="small" variant="primary" icon={Save} disabled={!dirty} loading={busy} onClick={() => void save()}>
            Save
          </Button>
        </>
      }
    >
      <Row top label="Roots">
        <StringList value={draft.roots} onChange={(roots) => set({ roots })} placeholder="~/workspace" validate={pathError} />
      </Row>
      <Row top label="Include">
        <StringList value={draft.include} onChange={(include) => set({ include })} placeholder="~/elsewhere/my-app" validate={pathError} />
      </Row>
      <Row top label="Exclude">
        <StringList value={draft.exclude} onChange={(exclude) => set({ exclude })} placeholder="~/workspace/old-thing" validate={pathError} />
      </Row>
      <Row top label="Extra roots" hint="Directories outside projects the editor may open read-only (Claude scratchpads, ~/.claude).">
        <StringList value={draft.extraRoots} onChange={(extraRoots) => set({ extraRoots })} placeholder="~/.claude" validate={pathError} />
      </Row>
    </Group>
  )
}

type LayerTab = 'repo' | 'overlay' | 'detected' | 'merged'

function ProjectConfig({ projectId }: { projectId: string }) {
  const qc = useQueryClient()
  const q = useProjectSettings(projectId)
  const [tab, setTab] = useState<LayerTab>('overlay')
  const [busy, setBusy] = useState(false)
  const repo = useDraft<string>(q.data ? (q.data.repoFile ?? '') : undefined)
  const overlay = useDraft<string>(q.data ? (q.data.overlayFile ?? '') : undefined)
  const layer = tab === 'repo' ? repo : tab === 'overlay' ? overlay : null
  const { diag, checking } = useTomlDiagnostics(layer ? layer.draft : null, 'project')

  if (q.error) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  if (!q.data || repo.draft === null || overlay.draft === null) return <Loading />
  const d = q.data

  const save = async (which: 'repo' | 'overlay', force = false) => {
    const l = which === 'repo' ? repo : overlay
    const text = l.draft ?? ''
    const existed = which === 'repo' ? d.repoFile !== null : d.overlayFile !== null
    if (!text.trim() && existed) {
      const ok = await confirmDialog({
        title: `Delete ${which === 'repo' ? d.repoPath : d.overlayPath}?`,
        message: 'The file is empty, so saving removes it.',
        confirmLabel: 'Delete file',
        danger: true,
      })
      if (!ok) return
    }
    setBusy(true)
    try {
      const r = await api.put<LayerSaveResult>(`/api/settings/projects/${encodeURIComponent(projectId)}/${which}`, {
        text,
        baseHash: force ? undefined : which === 'repo' ? d.repoHash : d.overlayHash,
      })
      toast('success', `${which === 'repo' ? '.workbench.toml' : 'Machine overlay'} saved; ${d.name} reloaded`)
      for (const w of [...r.warnings, ...r.projectWarnings]) toast('warning', w, { timeout: 9000 })
      await qc.invalidateQueries({ queryKey: pk.project(projectId) })
      void qc.invalidateQueries({ queryKey: qk.projects })
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toast('warning', 'The file changed on disk since you opened it.', {
          timeout: 0,
          action: { label: 'Overwrite it', run: () => void save(which, true) },
        })
      } else {
        toastError(e, 'Could not save')
      }
    } finally {
      setBusy(false)
    }
  }

  const tabs: { id: LayerTab; label: string; badge?: ReactNode }[] = [
    { id: 'overlay', label: 'Machine overlay', badge: overlay.dirty ? <span className="wb-warning">•</span> : undefined },
    { id: 'repo', label: '.workbench.toml', badge: repo.dirty ? <span className="wb-warning">•</span> : undefined },
    { id: 'detected', label: 'Detected' },
    { id: 'merged', label: 'Merged' },
  ]

  return (
    <div className="wb-set-box">
      <Tabs tabs={tabs} value={tab} onChange={setTab} />
      <div style={{ padding: 10, display: 'flex', flexDirection: 'column', gap: 8 }}>
        <div className="wb-small wb-muted">
          {tab === 'overlay' && (
            <>
              <span className="mono">{d.overlayPath}</span> — this machine only: hosts, secret references, local overrides. Highest priority.
            </>
          )}
          {tab === 'repo' && (
            <>
              <span className="mono">{d.repoPath}</span> — lives in the repository and can be committed. No secrets. Workbench writes it but never commits.
            </>
          )}
          {tab === 'detected' && <>What Workbench found in the repository (read-only; lowest priority).</>}
          {tab === 'merged' && <>The effective configuration: detected, then .workbench.toml, then the machine overlay.</>}
        </div>
        {layer ? (
          <>
            <TomlEditor
              kind="project"
              value={layer.draft ?? ''}
              onChange={(v) => layer.setDraft(v)}
              diagnostic={diag}
              onSave={() => void save(tab as 'repo' | 'overlay')}
            />
            <div className="wb-row">
              <DiagnosticLine diag={diag} checking={checking} />
              <span style={{ flex: 1 }} />
              <Button size="small" icon={RotateCcw} disabled={!layer.dirty || busy} onClick={layer.reset}>
                Revert
              </Button>
              <Button
                size="small"
                variant="primary"
                icon={Save}
                disabled={!layer.dirty || (diag !== null && !diag.ok)}
                loading={busy}
                onClick={() => void save(tab as 'repo' | 'overlay')}
              >
                Save and reload
              </Button>
            </div>
          </>
        ) : (
          <TomlEditor kind="project" readOnly value={tab === 'detected' ? d.detectedToml : d.mergedToml} />
        )}
        {d.warnings.length > 0 && (
          <Note tone="warning">
            {d.warnings.map((w) => (
              <div key={w}>{w}</div>
            ))}
          </Note>
        )}
      </div>
    </div>
  )
}

export function ProjectsSection() {
  const { data: projects, isLoading } = useProjects()
  const current = useUi((s) => s.projectId)
  const [picked, setPicked] = useState<string | null>(null)
  const projectId = picked ?? (projects?.some((p) => p.id === current) ? current : (projects?.[0]?.id ?? null))
  return (
    <Page title="Projects" description="Which directories are projects, and each project's configuration layers." wide>
      <DiscoveryGroup />
      <Group
        title="Project configuration"
        flush
        actions={
          projects && projects.length > 0 ? (
            <Select value={projectId ?? ''} onChange={(e) => setPicked(e.target.value)} style={{ height: 24 }}>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </Select>
          ) : undefined
        }
      >
        {isLoading ? (
          <Loading />
        ) : !projectId ? (
          <div className="wb-set-box">
            <EmptyState title="No projects">
              No git repository was found directly under the roots above. Clone one into a root and reload the projects (Ctrl+K → Reload
              projects), or list its directory under Include.
            </EmptyState>
          </div>
        ) : (
          <ProjectConfig key={projectId} projectId={projectId} />
        )}
      </Group>
    </Page>
  )
}
