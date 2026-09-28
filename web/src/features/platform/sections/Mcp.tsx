import { useState } from 'react'
import { RefreshCw } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { useUi } from '@/state/store'
import { Badge, EmptyState, ErrorBox, IconButton, Loading, Select } from '@/ui'
import { useMcpOverview } from '../api'
import { CodeLine, Group, Note, Page } from '../common'
import type { ClaudeMcpServer } from '../types'

const STATUS: Record<ClaudeMcpServer['status'], { tone?: 'success' | 'warning' | 'danger'; label: string }> = {
  enabled: { tone: 'success', label: 'enabled' },
  'needs-approval': { tone: 'warning', label: 'needs approval' },
  disabled: { label: 'disabled' },
  denied: { tone: 'danger', label: 'denied' },
}

export function McpSection() {
  const { data: projects } = useProjects()
  const current = useUi((s) => s.projectId)
  const [projectId, setProjectId] = useState<string | null>(current)
  const overview = useMcpOverview(projectId)
  const data = overview.data
  const writes = data?.workbench.tools.filter((t) => t.mutating).length ?? 0

  return (
    <Page
      title="MCP"
      wide
      description="Model Context Protocol servers give Claude sessions extra tools. Workbench is one of them; the others come from your Claude Code configuration."
    >
      {overview.error ? (
        <ErrorBox error={overview.error} onRetry={() => void overview.refetch()} />
      ) : !data ? (
        <Loading />
      ) : (
        <>
          <Group
            title={`Workbench tools (${data.workbench.tools.length})`}
            description={
              <>
                {data.workbench.note} {writes > 0 && <>Tools marked <Badge tone="warning">writes</Badge> change remote state.</>}
              </>
            }
            flush
          >
            <div style={{ marginBottom: 8, maxWidth: 520 }}>
              <CodeLine text={data.workbench.endpoint} />
            </div>
            <div className="wb-set-box">
              {data.workbench.tools.length === 0 ? (
                <EmptyState title="No tools registered" />
              ) : (
                <table className="wb-pf-table">
                  <thead>
                    <tr>
                      <th>Tool</th>
                      <th>Description</th>
                      <th />
                    </tr>
                  </thead>
                  <tbody>
                    {data.workbench.tools.map((t) => (
                      <tr key={t.name}>
                        <td className="mono" style={{ whiteSpace: 'nowrap' }}>
                          {t.name}
                        </td>
                        <td className="desc">{t.description}</td>
                        <td className="actions">{t.mutating ? <Badge tone="warning">writes</Badge> : <Badge>read</Badge>}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </Group>

          <Group
            title="Claude Code MCP servers"
            flush
            description="What Claude Code loads from ~/.claude.json, the project's .mcp.json, settings and plugins. Only names are shown for environment variables and headers — never values."
            actions={
              <>
                <Select value={projectId ?? ''} onChange={(e) => setProjectId(e.target.value || null)} style={{ height: 24 }}>
                  <option value="">User-level only</option>
                  {(projects ?? []).map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </Select>
                <IconButton icon={RefreshCw} size="small" label="Reload" onClick={() => void overview.refetch()} />
              </>
            }
          >
            <div className="wb-set-box" style={{ overflowX: 'auto' }}>
              {data.servers.length === 0 ? (
                <EmptyState title="No MCP servers configured" />
              ) : (
                <table className="wb-pf-table">
                  <thead>
                    <tr>
                      <th>Server</th>
                      <th>Scope</th>
                      <th>Transport</th>
                      <th>Endpoint / command</th>
                      <th>Credentials</th>
                      <th>Status</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.servers.map((s) => {
                      const st = STATUS[s.status] ?? { label: s.status }
                      const names = [...s.envNames.map((n) => `$${n}`), ...s.headerNames]
                      return (
                        <tr key={`${s.scope}:${s.name}`}>
                          <td className="mono">{s.name}</td>
                          <td>
                            <Badge title={s.source}>{s.scope}</Badge>
                          </td>
                          <td>{s.transport}</td>
                          <td className="mono wb-muted">
                            {s.endpoint ?? (s.command ? `${s.command}${s.args ? ` (+${s.args} args)` : ''}` : '—')}
                          </td>
                          <td className="mono wb-subtle wb-small">{names.length ? names.join(', ') : '—'}</td>
                          <td>
                            <Badge tone={st.tone} title={s.reason}>
                              {st.label}
                            </Badge>
                          </td>
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              )}
            </div>
          </Group>

          <div style={{ marginTop: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
            {data.notes.map((n) => (
              <Note key={n}>{n}</Note>
            ))}
            {data.accountConnectorsUsed && <Note>This Claude account has used claude.ai connectors; they are available in sessions signed in to it.</Note>}
            {data.files.some((f) => f.status === 'error') && (
              <Note tone="warning">
                Could not read:{' '}
                {data.files
                  .filter((f) => f.status === 'error')
                  .map((f) => `${f.path} (${f.error})`)
                  .join(', ')}
              </Note>
            )}
          </div>
        </>
      )}
    </Page>
  )
}
