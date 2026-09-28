// Create a Jira issue: project (defaults from [links.jira]), type (createmeta),
// summary, markdown description, labels.

import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useProject } from '@/api/queries'
import { useUi } from '@/state/store'
import { Button, ErrorBox, Field, Input, Modal, Select, TextArea } from '@/ui'
import { jiraApi, qk } from '../api'
import { useAtlassianUi } from '../state'
import { openJiraIssue } from '../confluence/actions'

export function CreateIssueDialog() {
  const req = useAtlassianUi((s) => s.createIssue)
  if (!req) return null
  return <CreateIssueForm projectKey={req.projectKey} onClose={() => useAtlassianUi.getState().openCreateIssue(null)} />
}

function CreateIssueForm({ projectKey, onClose }: { projectKey?: string; onClose: () => void }) {
  const qc = useQueryClient()
  const pid = useUi((s) => s.projectId)
  const project = useProject(pid)
  const projects = useQuery({ queryKey: qk.jiraProjects(pid), queryFn: () => jiraApi.projects(pid), staleTime: 10 * 60_000 })
  const linked = project.data?.config.links?.jira?.project_keys?.[0]?.toUpperCase()
  const [pkey, setPkey] = useState(projectKey ?? '')
  const effective = pkey || linked || projects.data?.[0]?.key || ''
  const types = useQuery({
    queryKey: qk.issueTypes(pid, effective),
    queryFn: () => jiraApi.issueTypes(pid, effective),
    enabled: !!effective,
    staleTime: 10 * 60_000,
  })
  const [typeId, setTypeId] = useState('')
  const [summary, setSummary] = useState('')
  const [description, setDescription] = useState('')
  const [labels, setLabels] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)

  useEffect(() => {
    const t = types.data?.find((x) => x.name.toLowerCase() === 'task') ?? types.data?.find((x) => !x.subtask)
    setTypeId(t?.id ?? '')
  }, [types.data])

  const create = async () => {
    if (!summary.trim() || !effective) return
    setBusy(true)
    setError(null)
    try {
      const out = await jiraApi.create(pid, {
        projectKey: effective,
        issueTypeId: typeId || undefined,
        summary: summary.trim(),
        description: description.trim() || undefined,
        labels: labels.split(/[\s,]+/).map((l) => l.trim()).filter(Boolean),
      })
      onClose()
      qc.invalidateQueries({ queryKey: ['jira', 'search'] })
      openJiraIssue(out.key, summary.trim())
    } catch (e) {
      setError(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      title="Create Jira issue"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!summary.trim() || !effective} onClick={() => void create()}>
            Create
          </Button>
        </>
      }
    >
      <div className="wb-row" style={{ gap: 8, alignItems: 'flex-start' }}>
        <div className="wb-grow">
          <Field label="Project">
            <Select value={effective} onChange={(e) => setPkey(e.target.value)}>
              {!projects.data?.length && effective && <option value={effective}>{effective}</option>}
              {(projects.data ?? []).map((p) => (
                <option key={p.key} value={p.key}>
                  {p.name} · {p.key}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <div className="wb-grow">
          <Field label="Type">
            <Select value={typeId} onChange={(e) => setTypeId(e.target.value)} disabled={!types.data}>
              {(types.data ?? []).map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </Select>
          </Field>
        </div>
      </div>
      <Field label="Summary">
        <Input autoFocus value={summary} onChange={(e) => setSummary(e.target.value)} placeholder="What needs doing?" />
      </Field>
      <Field label="Description (markdown)">
        <TextArea rows={7} value={description} onChange={(e) => setDescription(e.target.value)} />
      </Field>
      <Field label="Labels">
        <Input value={labels} onChange={(e) => setLabels(e.target.value)} placeholder="space or comma separated" />
      </Field>
      {projects.error && <ErrorBox error={projects.error} />}
      {types.error && <ErrorBox error={types.error} />}
      {error !== null && <ErrorBox error={error} />}
    </Modal>
  )
}
