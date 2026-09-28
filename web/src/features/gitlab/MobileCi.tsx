// Phone "CI" tab: pipelines → a pipeline's jobs → a job's log, with a back bar.

import { useEffect, useState, type ReactNode } from 'react'
import { ArrowLeft } from 'lucide-react'
import { EmptyState, ErrorBox, GitLabIcon, IconButton, Loading } from '@/ui'
import { useGitlabSummary, useHasGitlab, useJob, usePipeline } from './api'
import { StatusIcon, StatusText } from './components'
import { JobActions } from './JobPanel'
import { JobLogView, useJobTrace } from './JobLog'
import { PipelinesList } from './Lists'
import { PipelineGraph, PipelineHeader } from './PipelinePanel'

type View = { kind: 'list' } | { kind: 'pipeline'; id: number } | { kind: 'job'; id: number; pipelineId: number | null }

function Back({ onBack, children }: { onBack: () => void; children: ReactNode }) {
  return (
    <div className="gl-mobile-back">
      <IconButton icon={ArrowLeft} label="Back" onClick={onBack} />
      {children}
    </div>
  )
}

function MobileJob({ projectId, id, onBack }: { projectId: string; id: number; onBack: () => void }) {
  const job = useJob(projectId, id)
  const trace = useJobTrace(projectId, id, true)
  const j = job.data
  return (
    <div className="wb-fill">
      <Back onBack={onBack}>
        {j && <StatusIcon status={j.status} size={16} />}
        <span className="wb-grow wb-ellipsis" style={{ fontWeight: 600 }}>
          {j?.name ?? `Job ${id}`}
        </span>
        {j && <StatusText status={trace.status ?? j.status} />}
        {j && <JobActions projectId={projectId} job={j} compact />}
      </Back>
      {job.error && !j ? <ErrorBox error={job.error} /> : <JobLogView trace={trace} outline={false} />}
    </div>
  )
}

function MobilePipeline({ projectId, id, onBack, onJob }: { projectId: string; id: number; onBack: () => void; onJob: (id: number) => void }) {
  const q = usePipeline(projectId, id)
  const summary = useGitlabSummary(projectId)
  return (
    <div className="wb-fill">
      <Back onBack={onBack}>
        <span className="wb-grow wb-ellipsis" style={{ fontWeight: 600 }}>
          Pipeline #{q.data?.pipeline.iid ?? id}
        </span>
      </Back>
      {q.error && !q.data ? (
        <ErrorBox error={q.error} onRetry={() => q.refetch()} />
      ) : !q.data ? (
        <Loading />
      ) : (
        <div className="wb-scroll">
          <PipelineHeader projectId={projectId} d={q.data} compact />
          <PipelineGraph
            projectId={projectId}
            projectPath={summary.data?.path ?? projectId}
            stages={q.data.stages}
            vertical
            onOpenJob={(j) => onJob(j.id)}
          />
        </div>
      )}
    </div>
  )
}

export function MobileCi({ projectId }: { projectId: string | null }) {
  const has = useHasGitlab(projectId)
  const summary = useGitlabSummary(projectId, has)
  const [view, setView] = useState<View>({ kind: 'list' })
  useEffect(() => setView({ kind: 'list' }), [projectId])
  if (!projectId) return <EmptyState title="No project selected" />
  if (!has) return <EmptyState icon={GitLabIcon} title="This project is not on GitLab" />
  if (view.kind === 'job')
    return (
      <MobileJob
        projectId={projectId}
        id={view.id}
        onBack={() => setView(view.pipelineId ? { kind: 'pipeline', id: view.pipelineId } : { kind: 'list' })}
      />
    )
  if (view.kind === 'pipeline')
    return (
      <MobilePipeline
        projectId={projectId}
        id={view.id}
        onBack={() => setView({ kind: 'list' })}
        onJob={(jobId) => setView({ kind: 'job', id: jobId, pipelineId: view.id })}
      />
    )
  if (summary.error && !summary.data) return <ErrorBox error={summary.error} onRetry={() => summary.refetch()} />
  return (
    <div className="wb-fill gl-mobile">
      <PipelinesList projectId={projectId} summary={summary.data} compact onOpen={(p) => setView({ kind: 'pipeline', id: p.id })} />
    </div>
  )
}
