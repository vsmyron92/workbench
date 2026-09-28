// Global GitHub dialogs (mounted once by the feature's provider): create a pull
// request for the current branch, and run a workflow (workflow_dispatch, with
// the inputs its file declares).

import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { GitPullRequestCreate, Play } from 'lucide-react'
import { showToolWindow, toast, toastError } from '@/shell/actions'
import { Button, Checkbox, Field, Input, Loading, Modal, Select, TextArea } from '@/ui'
import { ghApi, ghk, useBranch, useDispatchInfo, useGithubSummary, useWorkflows } from './api'
import { openPr, useGhUi } from './components'
import { titleFromBranch } from './logic'

function CreatePrDialog({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const qc = useQueryClient()
  const summary = useGithubSummary(projectId)
  const s = summary.data
  const [head, setHead] = useState('')
  const [base, setBase] = useState('')
  const [title, setTitle] = useState('')
  const [titleTouched, setTitleTouched] = useState(false)
  const [body, setBody] = useState('')
  const [draft, setDraft] = useState(false)
  const [busy, setBusy] = useState(false)
  const branch = useBranch(projectId, head.trim() || null)

  useEffect(() => {
    if (!s) return
    setHead((v) => v || s.branch || '')
    setBase((v) => v || s.defaultBranch || '')
  }, [s?.fetchedAt]) // eslint-disable-line react-hooks/exhaustive-deps

  // Suggest the branch head's commit subject (or the branch name) as the title.
  useEffect(() => {
    if (titleTouched || !head.trim()) return
    setTitle(branch.data?.title || titleFromBranch(head.trim()))
  }, [branch.data, head, titleTouched])

  const existing = s?.currentPr && s.currentPr.state === 'open' && s.currentPr.head?.ref === head.trim() ? s.currentPr : null
  const missing = branch.data && !branch.data.exists
  const same = head.trim() !== '' && head.trim() === base.trim()
  const canCreate = !!title.trim() && !!head.trim() && !!base.trim() && !same && !missing && !existing && !busy

  const create = async () => {
    setBusy(true)
    try {
      const pr = await ghApi.createPr(projectId, { head: head.trim(), base: base.trim(), title: title.trim(), body: body.trim() || undefined, draft })
      toast('success', `Created #${pr.number}`)
      void qc.invalidateQueries({ queryKey: ghk.pulls(projectId) })
      void qc.invalidateQueries({ queryKey: ghk.summary(projectId) })
      openPr(projectId, pr.number, pr.title)
      onClose()
    } catch (e) {
      toastError(e, 'Could not create the pull request')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      wide
      title="Create pull request"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={GitPullRequestCreate} loading={busy} disabled={!canCreate} onClick={create}>
            {draft ? 'Create draft pull request' : 'Create pull request'}
          </Button>
        </>
      }
    >
      <div className="wb-row" style={{ gap: 10, alignItems: 'flex-start' }}>
        <div className="wb-grow">
          <Field
            label="Branch (head)"
            hint={
              missing ? (
                <span className="wb-warning">{head.trim()} is not on GitHub yet: push it first.</span>
              ) : branch.data?.sha ? (
                <span>
                  Head <span className="gh-mono">{branch.data.sha.slice(0, 7)}</span> {branch.data.title}
                </span>
              ) : undefined
            }
          >
            <Input value={head} onChange={(e) => setHead(e.target.value)} spellCheck={false} />
          </Field>
        </div>
        <div className="wb-grow">
          <Field label="Into (base)" hint={same ? <span className="wb-warning">Head and base are the same branch.</span> : undefined}>
            <Input value={base} onChange={(e) => setBase(e.target.value)} spellCheck={false} />
          </Field>
        </div>
      </div>
      {existing && (
        <div className="gh-banner info" style={{ borderRadius: 'var(--radius)' }}>
          <span className="wb-grow">
            #{existing.number} {existing.title} is already open for this branch.
          </span>
          <Button
            size="small"
            onClick={() => {
              openPr(projectId, existing.number, existing.title)
              onClose()
            }}
          >
            Open it
          </Button>
        </div>
      )}
      <Field label="Title">
        <Input
          value={title}
          onChange={(e) => {
            setTitle(e.target.value)
            setTitleTouched(true)
          }}
          autoFocus
        />
      </Field>
      <Field label="Description" hint="Markdown">
        <TextArea rows={8} value={body} onChange={(e) => setBody(e.target.value)} placeholder="What changes, and why" />
      </Field>
      <Checkbox checked={draft} onChange={setDraft}>
        Create as a draft
      </Checkbox>
    </Modal>
  )
}

function RunWorkflowDialog({ projectId, initial, onClose }: { projectId: string; initial: number | null; onClose: () => void }) {
  const qc = useQueryClient()
  const setTab = useGhUi((s) => s.setTab)
  const summary = useGithubSummary(projectId)
  const workflows = useWorkflows(projectId)
  const active = useMemo(() => (workflows.data ?? []).filter((w) => w.state === 'active'), [workflows.data])
  const [workflowId, setWorkflowId] = useState<number | null>(initial)
  const [ref, setRef] = useState('')
  const [values, setValues] = useState<Record<string, string | boolean>>({})
  const [busy, setBusy] = useState(false)
  const wid = workflowId ?? active[0]?.id ?? null
  const info = useDispatchInfo(projectId, wid, ref.trim())

  useEffect(() => {
    if (summary.data) setRef((r) => r || summary.data!.branch || summary.data!.defaultBranch || '')
  }, [summary.data])
  // Defaults from the workflow file.
  useEffect(() => {
    const next: Record<string, string | boolean> = {}
    for (const i of info.data?.inputs ?? []) {
      if (i.type === 'boolean') next[i.name] = i.default === 'true'
      else next[i.name] = i.default ?? (i.type === 'choice' ? (i.options[0] ?? '') : '')
    }
    setValues(next)
  }, [info.data])

  const missingRequired = (info.data?.inputs ?? []).some((i) => i.required && i.type !== 'boolean' && !String(values[i.name] ?? '').trim())
  const run = async () => {
    if (wid === null) return
    setBusy(true)
    try {
      await ghApi.dispatch(projectId, wid, ref.trim(), values)
      const name = active.find((w) => w.id === wid)?.name ?? 'the workflow'
      toast('success', `Started ${name} on ${ref.trim()}`)
      // The run appears on GitHub a moment later.
      window.setTimeout(() => void qc.invalidateQueries({ queryKey: ghk.runs(projectId) }), 3000)
      setTab('runs')
      showToolWindow('github')
      onClose()
    } catch (e) {
      toastError(e, 'Could not start the workflow')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      title="Run workflow"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={Play} loading={busy} disabled={!ref.trim() || wid === null || !info.data?.dispatchable || missingRequired} onClick={run}>
            Run workflow
          </Button>
        </>
      }
    >
      <Field label="Workflow">
        {workflows.isLoading ? (
          <Loading />
        ) : (
          <Select value={wid ?? ''} onChange={(e) => setWorkflowId(Number(e.target.value))}>
            {active.map((w) => (
              <option key={w.id} value={w.id}>
                {w.name}
              </option>
            ))}
          </Select>
        )}
      </Field>
      <Field label="Branch">
        <Input value={ref} onChange={(e) => setRef(e.target.value)} spellCheck={false} />
      </Field>
      {info.isLoading && wid !== null && <Loading label="Reading the workflow…" />}
      {info.data && !info.data.dispatchable && (
        <div className="wb-small wb-warning">This workflow has no workflow_dispatch trigger on {ref.trim() || 'this branch'}, so it cannot be started by hand.</div>
      )}
      {info.data?.inputs.map((i) =>
        i.type === 'boolean' ? (
          <Checkbox key={i.name} checked={!!values[i.name]} onChange={(v) => setValues({ ...values, [i.name]: v })}>
            <span className="gh-mono">{i.name}</span>
            {i.description && <span className="wb-muted"> — {i.description}</span>}
          </Checkbox>
        ) : (
        <Field key={i.name} label={`${i.name}${i.required ? ' *' : ''}`} hint={i.description ?? undefined}>
          {i.type === 'choice' && i.options.length ? (
            <Select value={String(values[i.name] ?? '')} onChange={(e) => setValues({ ...values, [i.name]: e.target.value })}>
              {i.options.map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </Select>
          ) : (
            <Input
              value={String(values[i.name] ?? '')}
              type={i.type === 'number' ? 'number' : 'text'}
              onChange={(e) => setValues({ ...values, [i.name]: e.target.value })}
              spellCheck={false}
            />
          )}
        </Field>
        ),
      )}
    </Modal>
  )
}

export function GithubDialogs() {
  const createPr = useGhUi((s) => s.createPr)
  const runWorkflow = useGhUi((s) => s.runWorkflow)
  const close = useGhUi((s) => s.closeDialogs)
  return (
    <>
      {createPr && <CreatePrDialog projectId={createPr} onClose={close} />}
      {runWorkflow && <RunWorkflowDialog projectId={runWorkflow.pid} initial={runWorkflow.workflowId} onClose={close} />}
    </>
  )
}
