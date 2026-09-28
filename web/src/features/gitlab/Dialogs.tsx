// Global GitLab dialogs (mounted once by the feature's provider): create a
// merge request for the current branch, and run a pipeline.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { GitPullRequestCreate, Play, Plus, Trash2 } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import { Button, Checkbox, Field, IconButton, Input, Modal, TextArea } from '@/ui'
import { glApi, glk, useBranch, useGitlabSummary } from './api'
import { openMr, openPipeline, useGlUi } from './components'

/** "feature/add-thing_now" → "Add thing now" */
export function titleFromBranch(branch: string): string {
  const last = branch.split('/').pop() ?? branch
  const words = last.replace(/[-_]+/g, ' ').trim()
  return words ? words[0].toUpperCase() + words.slice(1) : branch
}

function CreateMrDialog({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const qc = useQueryClient()
  const summary = useGitlabSummary(projectId)
  const s = summary.data
  const [source, setSource] = useState('')
  const [target, setTarget] = useState('')
  const [title, setTitle] = useState('')
  const [titleTouched, setTitleTouched] = useState(false)
  const [description, setDescription] = useState('')
  const [draft, setDraft] = useState(false)
  const [removeSource, setRemoveSource] = useState(true)
  const [squash, setSquash] = useState(false)
  const [busy, setBusy] = useState(false)
  const branch = useBranch(projectId, source.trim() || null)

  // Defaults from the summary once it is there.
  useEffect(() => {
    if (!s) return
    setSource((v) => v || s.branch || '')
    setTarget((v) => v || s.defaultBranch || '')
    setRemoveSource(s.removeSourceBranchAfterMerge ?? true)
    setSquash(s.squashOption === 'always' || s.squashOption === 'default_on')
  }, [s?.fetchedAt]) // eslint-disable-line react-hooks/exhaustive-deps

  // Suggest the branch head's commit subject (or the branch name) as the title.
  useEffect(() => {
    if (titleTouched || !source.trim()) return
    setTitle(branch.data?.commit?.title || titleFromBranch(source.trim()))
  }, [branch.data, source, titleTouched])

  const existing = s?.currentMr && s.currentMr.state === 'opened' && s.currentMr.sourceBranch === source.trim() ? s.currentMr : null
  const missing = branch.data && !branch.data.exists
  const same = source.trim() !== '' && source.trim() === target.trim()
  const canCreate = !!title.trim() && !!source.trim() && !!target.trim() && !same && !missing && !existing && !busy

  const create = async () => {
    setBusy(true)
    try {
      const mr = await glApi.createMr(projectId, {
        sourceBranch: source.trim(),
        targetBranch: target.trim(),
        title: title.trim(),
        description: description.trim() || undefined,
        draft,
        removeSourceBranch: removeSource,
        squash,
      })
      toast('success', `Created !${mr.iid}`)
      qc.invalidateQueries({ queryKey: glk.mrs(projectId) })
      qc.invalidateQueries({ queryKey: glk.summary(projectId) })
      openMr(projectId, mr.iid, mr.title)
      onClose()
    } catch (e) {
      toastError(e, 'Could not create the merge request')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      wide
      title="Create merge request"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={GitPullRequestCreate} loading={busy} disabled={!canCreate} onClick={create}>
            Create merge request
          </Button>
        </>
      }
    >
      <div className="wb-row" style={{ gap: 10, alignItems: 'flex-start' }}>
        <div className="wb-grow">
          <Field
            label="Source branch"
            hint={
              missing ? (
                <span className="wb-warning">{source.trim()} is not on GitLab yet: push it first.</span>
              ) : branch.data?.commit ? (
                <span>
                  Head <span className="gl-mono">{branch.data.commit.shortId}</span> {branch.data.commit.title}
                </span>
              ) : undefined
            }
          >
            <Input value={source} onChange={(e) => setSource(e.target.value)} spellCheck={false} />
          </Field>
        </div>
        <div className="wb-grow">
          <Field label="Target branch" hint={same ? <span className="wb-warning">Source and target are the same branch.</span> : undefined}>
            <Input value={target} onChange={(e) => setTarget(e.target.value)} spellCheck={false} />
          </Field>
        </div>
      </div>
      {existing && (
        <div className="gl-banner info" style={{ borderRadius: 'var(--radius)' }}>
          <span className="wb-grow">
            !{existing.iid} {existing.title} is already open for this branch.
          </span>
          <Button
            size="small"
            onClick={() => {
              openMr(projectId, existing.iid, existing.title)
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
        <TextArea rows={8} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What changes, and why" />
      </Field>
      <div className="wb-row" style={{ gap: 16, flexWrap: 'wrap' }}>
        <Checkbox checked={draft} onChange={setDraft}>
          Mark as draft
        </Checkbox>
        <Checkbox checked={removeSource} onChange={setRemoveSource}>
          Delete source branch when merged
        </Checkbox>
        <Checkbox checked={squash} onChange={setSquash} disabled={s?.squashOption === 'always' || s?.squashOption === 'never'}>
          Squash commits when merged
        </Checkbox>
      </div>
    </Modal>
  )
}

function RunPipelineDialog({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const qc = useQueryClient()
  const summary = useGitlabSummary(projectId)
  const [ref, setRef] = useState('')
  const [vars, setVars] = useState<{ key: string; value: string }[]>([])
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    if (summary.data) setRef((r) => r || summary.data!.branch || summary.data!.defaultBranch || '')
  }, [summary.data])
  const run = async () => {
    setBusy(true)
    try {
      const p = await glApi.runPipeline(
        projectId,
        ref.trim(),
        vars.filter((v) => v.key.trim()),
      )
      toast('success', `Started pipeline #${p.iid ?? p.id} on ${p.ref}`)
      qc.invalidateQueries({ queryKey: glk.pipelines(projectId) })
      openPipeline(projectId, p.id, p.iid)
      onClose()
    } catch (e) {
      toastError(e, 'Could not start the pipeline')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Run pipeline"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={Play} loading={busy} disabled={!ref.trim()} onClick={run}>
            Run pipeline
          </Button>
        </>
      }
    >
      <Field label="Branch or tag">
        <Input value={ref} onChange={(e) => setRef(e.target.value)} spellCheck={false} autoFocus />
      </Field>
      <Field label="Variables">
        <div className="wb-fill" style={{ gap: 4, height: 'auto' }}>
          {vars.map((v, i) => (
            <div className="wb-row" key={i}>
              <Input
                className="wb-grow gl-mono"
                placeholder="KEY"
                value={v.key}
                onChange={(e) => setVars(vars.map((x, j) => (j === i ? { ...x, key: e.target.value } : x)))}
              />
              <Input className="wb-grow" placeholder="value" value={v.value} onChange={(e) => setVars(vars.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)))} />
              <IconButton icon={Trash2} size="small" label="Remove" onClick={() => setVars(vars.filter((_, j) => j !== i))} />
            </div>
          ))}
          <div>
            <Button size="small" variant="ghost" icon={Plus} onClick={() => setVars([...vars, { key: '', value: '' }])}>
              Add variable
            </Button>
          </div>
        </div>
      </Field>
    </Modal>
  )
}

export function GitlabDialogs() {
  const createMr = useGlUi((s) => s.createMr)
  const runPipeline = useGlUi((s) => s.runPipeline)
  const close = useGlUi((s) => s.closeDialogs)
  return (
    <>
      {createMr && <CreateMrDialog projectId={createMr} onClose={close} />}
      {runPipeline && <RunPipelineDialog projectId={runPipeline} onClose={close} />}
    </>
  )
}
