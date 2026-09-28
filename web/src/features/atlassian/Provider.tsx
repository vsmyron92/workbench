// Mounted once while signed in: keeps the Atlassian status in a store (tool-window
// `when` predicates read it synchronously), keeps caches fresh from server events,
// and hosts the slice's dialogs.

import { useEffect, type ReactNode } from 'react'
import { ApiError } from '@/api/client'
import { useProjects } from '@/api/queries'
import { useUi } from '@/state/store'
import { useAtlassianInvalidation, useAtlassianStatusQuery } from './api'
import { NewPageDialog } from './confluence/NewPageDialog'
import { CopyDialog, MoveDialog } from './confluence/PageOps'
import { CreateIssueDialog } from './jira/CreateIssueDialog'
import { useAtlassianStatus, useAtlassianUi } from './state'
import './atlassian.css'

export function AtlassianProvider({ children }: { children?: ReactNode }) {
  const projectId = useUi((s) => s.projectId)
  const { data: projects } = useProjects()
  const project = projects?.find((p) => p.id === projectId)
  // Probe only where Atlassian is configured (global site or the project's links);
  // elsewhere the status call would just answer "not configured" on every load.
  const q = useAtlassianStatusQuery(projectId, !!project && (project.hasConfluence || project.hasJira))
  useAtlassianInvalidation()

  useEffect(() => {
    const e = q.error
    // "Not set up" arrives as data (configured: false), not as an HTTP error.
    const unconfigured = q.data && !q.data.configured ? q.data : null
    useAtlassianStatus.getState().set({
      status: q.data ?? null,
      errorCode: e instanceof ApiError ? e.code : e ? 'error' : unconfigured ? 'not_configured' : null,
      errorMessage: e instanceof Error ? e.message : (unconfigured?.error ?? null),
    })
  }, [q.data, q.error])

  // Tool windows appear or disappear with Jira/Confluence availability; the shell
  // re-evaluates `when` on its next render, so give it one.
  const jira = !!q.data?.jira
  const confluence = !!q.data?.confluence
  useEffect(() => {
    useUi.setState((s) => ({ sides: { ...s.sides } }))
  }, [jira, confluence])

  return (
    <>
      {children}
      <NewPageDialog />
      <CreateIssueDialog />
      <PageOpDialogs />
    </>
  )
}

function PageOpDialogs() {
  const op = useAtlassianUi((s) => s.pageOp)
  if (!op) return null
  const close = () => useAtlassianUi.getState().openPageOp(null)
  return op.kind === 'move' ? (
    <MoveDialog key={op.page.id} projectId={op.projectId} page={op.page} onClose={close} />
  ) : (
    <CopyDialog key={op.page.id} projectId={op.projectId} page={op.page} onClose={close} />
  )
}
