// Pure helpers: parsing page/issue references, project link config, JQL filters,
// agent prompts. No React, no DOM (unit-tested in links.test.ts).

import type { ProjectConfig } from '@/api/types'

/** A Confluence page id from `458753`, a page URL, a `viewpage.action?pageId=` URL or an edit URL. */
export function parsePageRef(input: string): string | null {
  const s = input.trim()
  if (/^\d{1,20}$/.test(s)) return s
  const m = /\/pages\/(?:edit-v2\/)?(\d+)|[?&]pageId=(\d+)/.exec(s)
  return m ? (m[1] ?? m[2]) : null
}

/** A Jira issue key from `abc-12`, `ABC-12` or a `/browse/ABC-12` URL. */
export function parseIssueKey(input: string): string | null {
  const s = input.trim()
  const m = /(?:^|\/browse\/|selectedIssue=)([A-Za-z][A-Za-z0-9_]{0,30}-\d{1,12})(?:$|[/?#&])/.exec(s)
  return m ? m[1].toUpperCase() : null
}

export interface ConfluenceLinks {
  site: string | null
  space: string | null
  rootPages: string[]
  pinned: { name: string; id: string }[]
  archived: boolean
}

export function confluenceLinks(config: ProjectConfig | undefined | null): ConfluenceLinks {
  const c = config?.links?.confluence
  return {
    site: c?.site || null,
    space: c?.space || null,
    rootPages: (c?.root_pages ?? []).map(String),
    pinned: Object.entries(c?.pinned ?? {}).map(([name, id]) => ({ name, id: String(id) })),
    archived: !!c?.archived,
  }
}

export interface JqlFilter {
  id: string
  label: string
  jql: string
}

/** Saved filters: assigned to me, the project's own issues ([links.jira]), and recent activity. */
export function jqlFilters(config: ProjectConfig | undefined | null): JqlFilter[] {
  const j = config?.links?.jira
  const out: JqlFilter[] = [
    { id: 'mine', label: 'Assigned to me', jql: 'assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC' },
  ]
  const keys = (j?.project_keys ?? []).filter((k) => /^[A-Za-z][A-Za-z0-9_]*$/.test(k))
  if (j?.jql) out.push({ id: 'project', label: 'Project filter', jql: j.jql })
  if (keys.length) {
    const inList = keys.map((k) => k.toUpperCase()).join(', ')
    out.push({ id: 'open', label: `Open in ${inList}`, jql: `project in (${inList}) AND statusCategory != Done ORDER BY updated DESC` })
    out.push({ id: 'recent', label: `Recently updated in ${inList}`, jql: `project in (${inList}) AND updated >= -14d ORDER BY updated DESC` })
  }
  out.push({ id: 'reported', label: 'Reported by me', jql: 'reporter = currentUser() ORDER BY created DESC' })
  out.push({ id: 'watching', label: 'Recently updated (all)', jql: 'updated >= -7d ORDER BY updated DESC' })
  return out
}

export type Tone = 'success' | 'warning' | 'danger' | 'accent' | undefined

/** Jira status category → badge tone (To Do grey, In Progress blue, Done green). */
export function statusTone(category: string | null | undefined): Tone {
  if (category === 'done') return 'success'
  if (category === 'indeterminate') return 'accent'
  return undefined
}

/** Confluence status-macro colour → badge tone. */
export function lozengeTone(colour: string | null | undefined): Tone {
  switch ((colour ?? '').toLowerCase()) {
    case 'green':
      return 'success'
    case 'red':
      return 'danger'
    case 'yellow':
      return 'warning'
    case 'blue':
    case 'purple':
      return 'accent'
    default:
      return undefined
  }
}

export function pagePrompt(p: { id: string; title: string; version?: number; webUrl: string }, instruction: string): string {
  const known = !!p.version && p.version > 0
  return [
    `Confluence page "${p.title}" (page id ${p.id}${known ? `, version ${p.version}` : ''})${p.webUrl ? `: ${p.webUrl}` : ''}`,
    '',
    instruction.trim(),
    '',
    `Read it with the Workbench MCP tool confluence_get_page (format=storage when you will edit it). ` +
      `Save changes with confluence_update_page, passing baseVersion=${known ? p.version : '<the version confluence_get_page reports>'} and a short message. ` +
      'Keep every <ac:inline-comment-marker> element so inline comments stay anchored.',
  ].join('\n')
}

export function issuePrompt(i: { key: string; summary: string; webUrl: string; descriptionMarkdown: string }): string {
  const desc = i.descriptionMarkdown.trim()
  return [
    `Work on Jira issue ${i.key}: ${i.summary}`,
    i.webUrl,
    '',
    desc ? `Description:\n${desc.length > 6000 ? desc.slice(0, 6000) + '\n…(truncated; use jira_get_issue for the rest)' : desc}` : 'The issue has no description.',
    '',
    'Use the Workbench MCP tools jira_get_issue for details, jira_comment to report progress and jira_transition when the work is done.',
  ].join('\n')
}

/** Label names from what was typed ("a, b c" → a, b, c), lowercased like Confluence. */
export function parseLabels(text: string): string[] {
  return [...new Set(text.split(/[\s,]+/).map((l) => l.trim().toLowerCase()).filter(Boolean))]
}

/** Human file size (1.2 MB). */
export function fileSize(n: number | null | undefined): string {
  if (n === null || n === undefined) return ''
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

/** Initials for an avatar chip. */
export function initials(name: string | null | undefined): string {
  const parts = (name ?? '').trim().split(/\s+/).filter(Boolean)
  if (!parts.length) return '?'
  return (parts[0][0] + (parts.length > 1 ? parts[parts.length - 1][0] : '')).toUpperCase()
}

/** Bounded most-recently-used list (newest first, unique by id). */
export function pushRecent<T extends { id: string }>(list: T[], item: T, max = 15): T[] {
  return [item, ...list.filter((x) => x.id !== item.id)].slice(0, max)
}

/**
 * The part of a "not set up" message worth showing above the setup snippet. The server
 * appends its own copy of the config.toml snippet ("… Add to config.toml:\n\n[atlassian]
 * …") for agents and plain clients; the UI shows a formatted one instead.
 */
export function setupSummary(message: string | null | undefined): string {
  const m = (message ?? '').trim()
  const i = m.indexOf('Add to config.toml:')
  const head = (i >= 0 ? m.slice(0, i) : m).trim()
  return head || 'Atlassian is not set up.'
}
