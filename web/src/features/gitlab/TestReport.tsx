// The pipeline panel's Tests tab: the failed and errored cases of the pipeline's
// JUnit test report, by suite, each with its output (failure message and stack
// trace), Open File, Copy and Ask agent to fix.

import { useState } from 'react'
import { Bot, ChevronDown, ChevronRight, CircleX, Copy, FileCode, OctagonAlert, TriangleAlert } from 'lucide-react'
import { fileInProject, repoNote, scopeProject } from '@/api/repos'
import { askAgent } from '@/shell/agentBridge'
import { openPanel, toast, toastError } from '@/shell/actions'
import { Badge, Button, EmptyState, ErrorBox, formatDuration, IconButton, Loading } from '@/ui'
import { useTestFailures } from './api'
import { testFixPrompt } from './logic'
import type { FailedCase, Pipeline, SuiteFailures } from './types'

/** A report path as a project path (`./spec/a_spec.rb` → `spec/a_spec.rb`; in a repository below the root, `<repo>/spec/a_spec.rb`). */
function projectPath(scope: string, file: string): string {
  return fileInProject(scope, file)
}

function CaseRow({ projectId, repoPath, pipeline, suite, c }: { projectId: string; repoPath: string; pipeline: Pipeline; suite: string; c: FailedCase }) {
  const [open, setOpen] = useState(false)
  const Icon = c.status === 'error' ? OctagonAlert : CircleX
  const ask = async () => {
    try {
      await askAgent({ projectId: scopeProject(projectId), prompt: testFixPrompt({ projectPath: repoPath, pipeline, suite, test: c }) + repoNote(projectId) })
    } catch (e) {
      toastError(e, 'Could not ask an agent')
    }
  }
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(c.output)
      toast('success', 'Output copied')
    } catch {
      toast('warning', 'The browser refused the clipboard')
    }
  }
  return (
    <div className={`gl-tcase${open ? ' open' : ''}`}>
      <div className="gl-tcase-head" onClick={() => setOpen(!open)} role="button" aria-expanded={open}>
        {open ? <ChevronDown size={13} className="wb-subtle" /> : <ChevronRight size={13} className="wb-subtle" />}
        <Icon size={14} className="wb-danger" />
        <span className="gl-tcase-name wb-ellipsis" title={`${c.classname} › ${c.name}`}>
          {c.classname && <span className="wb-subtle">{c.classname} › </span>}
          {c.name}
        </span>
        {c.status === 'error' && <Badge tone="danger">error</Badge>}
        {c.recentFailures ? (
          <Badge tone="warning" title={`Also failed ${c.recentFailures} times on ${c.baseBranch ?? 'the base branch'} in the last 14 days (GitLab): maybe not this change`}>
            {c.recentFailures}× on {c.baseBranch ?? 'base'}
          </Badge>
        ) : null}
        <span className="wb-grow" />
        <span className="wb-subtle wb-xs">{formatDuration(c.time)}</span>
      </div>
      {open && (
        <div className="gl-tcase-body">
          <div className="gl-tcase-actions">
            <Button size="small" icon={Bot} onClick={() => void ask()}>
              Ask agent to fix
            </Button>
            {c.file && (
              <Button
                size="small"
                icon={FileCode}
                onClick={() => {
                  const path = projectPath(projectId, c.file!)
                  const real = scopeProject(projectId)
                  openPanel({ kind: 'editor', id: `editor:${real}:${path}`, params: { projectId: real, path } })
                }}
              >
                {projectPath(projectId, c.file)}
              </Button>
            )}
            <span className="wb-grow" />
            {c.output && <IconButton icon={Copy} size="small" label="Copy the output" onClick={() => void copy()} />}
          </div>
          {c.output ? <pre className="gl-tcase-output">{c.output}</pre> : <div className="wb-subtle wb-small">The report has no output for this test.</div>}
          {c.outputTruncated && <div className="wb-subtle wb-xs">The output is cut; the whole of it is in the job log.</div>}
        </div>
      )}
    </div>
  )
}

function Suite({ projectId, repoPath, pipeline, s }: { projectId: string; repoPath: string; pipeline: Pipeline; s: SuiteFailures }) {
  return (
    <div className="gl-tsuite">
      <div className="gl-tsuite-head">
        <b>{s.name}</b>
        <span className="wb-danger wb-small">
          {s.failedCount + s.errorCount} of {s.totalCount} failed
        </span>
        {s.skippedCount > 0 && <span className="wb-subtle wb-small">{s.skippedCount} skipped</span>}
        <span className="wb-grow" />
        <span className="wb-subtle wb-xs">{formatDuration(s.time)}</span>
      </div>
      {s.suiteError && (
        <div className="gl-tsuite-error">
          <TriangleAlert size={13} /> {s.suiteError}
        </div>
      )}
      {s.cases.map((c, i) => (
        <CaseRow key={`${c.classname}:${c.name}:${i}`} projectId={projectId} repoPath={repoPath} pipeline={pipeline} suite={s.name} c={c} />
      ))}
    </div>
  )
}

export function TestReport({ projectId, repoPath, pipeline }: { projectId: string; repoPath: string; pipeline: Pipeline }) {
  const q = useTestFailures(projectId, pipeline.id, true, pipeline.updatedAt)
  if (q.error) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  if (!q.data) return <Loading label="Reading the test report…" />
  const failing = q.data.suites.filter((s) => s.cases.length || s.suiteError)
  if (!failing.length) {
    return (
      <EmptyState icon={CircleX} title="No failed tests">
        {q.data.total.count} tests ran; none failed.
      </EmptyState>
    )
  }
  return (
    <div className="wb-scroll gl-treport">
      {failing.map((s) => (
        <Suite key={s.name} projectId={projectId} repoPath={repoPath} pipeline={pipeline} s={s} />
      ))}
      {q.data.truncated && <div className="wb-subtle wb-small gl-treport-note">Only the first 200 failures are listed; GitLab has the rest.</div>}
    </div>
  )
}
