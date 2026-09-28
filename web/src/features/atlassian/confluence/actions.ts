// Opening Confluence/Jira panels and small cross-component actions.

import { openPanel, promptDialog, toast } from '@/shell/actions'
import { askAgent } from '@/shell/agentBridge'
import { pagePrompt, parseIssueKey, parsePageRef } from '../links'
import { useAtlassianStatus } from '../state'

export function openConfluencePage(pageId: string, title?: string, opts: { mode?: 'view' | 'edit'; focus?: boolean } = {}) {
  openPanel({
    kind: 'confluence',
    id: `confluence:${pageId}`,
    title: title ?? `Page ${pageId}`,
    params: opts.mode ? { pageId, mode: opts.mode } : { pageId },
    focus: opts.focus,
  })
}

export function openJiraIssue(key: string, title?: string) {
  openPanel({ kind: 'jira', id: `jira:${key}`, title: title ? `${key} ${title}` : key, params: { key } })
}

export function openJiraBoard(boardId: number, title?: string) {
  openPanel({ kind: 'jira.board', id: `jira.board:${boardId}`, title: title ?? `Board ${boardId}`, params: { boardId } })
}

/** A browser URL for a page when only its id is known (Confluence redirects it). */
export function pageUrl(pageId: string): string | null {
  const site = useAtlassianStatus.getState().status?.site
  return site ? `${site}/wiki/pages/viewpage.action?pageId=${encodeURIComponent(pageId)}` : null
}

export function openExternal(url: string | null | undefined) {
  if (url) window.open(url, '_blank', 'noopener,noreferrer')
}

export async function copyText(text: string, what = 'Link') {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', `${what} copied`)
  } catch {
    toast('warning', 'Clipboard is not available here')
  }
}

export async function promptOpenPage() {
  const v = await promptDialog({
    title: 'Open Confluence page',
    label: 'Page ID or URL',
    placeholder: 'https://…/wiki/spaces/KEY/pages/123/Title  or  123',
    confirmLabel: 'Open',
  })
  if (!v) return
  const id = parsePageRef(v)
  if (id) openConfluencePage(id)
  else toast('warning', 'That is not a Confluence page ID or URL')
}

export async function promptOpenIssue() {
  const v = await promptDialog({ title: 'Open Jira issue', label: 'Issue key or URL', placeholder: 'ABC-123', confirmLabel: 'Open' })
  if (!v) return
  const key = parseIssueKey(v)
  if (key) openJiraIssue(key)
  else toast('warning', 'That is not a Jira issue key or URL')
}

/** Ask the project's agent to work on a page (the prompt tells it which MCP tools to use). */
export async function askAgentAboutPage(
  projectId: string | null,
  page: { id: string; title: string; version?: number; webUrl?: string | null },
) {
  const instruction = await promptDialog({
    title: `Ask agent about “${page.title}”`,
    label: 'What should the agent do with this page?',
    placeholder: 'e.g. Summarize the open questions, or tighten section 4 and fix the tables',
    multiline: true,
    confirmLabel: 'Ask agent',
  })
  if (!instruction?.trim()) return
  const prompt = pagePrompt({ id: page.id, title: page.title, version: page.version, webUrl: page.webUrl ?? pageUrl(page.id) ?? '' }, instruction)
  await askAgent({ projectId, prompt })
}
