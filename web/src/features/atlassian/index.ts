// Feature slice: atlassian. Owned by the atlassian slice — see docs/ARCHITECTURE.md.
//
// Confluence: 'confluence' tool window (right) and panel {pageId, mode?}; phone tab
// 'docs'. Jira (only when the site has it): 'jira' tool window (issues and boards),
// panels 'jira' {key} and 'jira.board' {boardId}.

import { BookOpen, FilePlus, FileSearch, FileText, Kanban, ListTodo, SquarePlus } from 'lucide-react'
import { configFileHint } from '@/api/health'
import { ConfluenceIcon, JiraIcon } from '@/ui'
import { showToolWindow, toast } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { useUi } from '@/state/store'
import { promptOpenIssue, promptOpenPage } from './confluence/actions'
import { MobileDocs } from './confluence/MobileDocs'
import { ConfluencePanel } from './confluence/PagePanel'
import { ConfluenceToolWindow } from './confluence/ToolWindow'
import { BoardPanel } from './jira/BoardPanel'
import { JiraPanel } from './jira/IssuePanel'
import { JiraToolWindow } from './jira/JiraToolWindow'
import { confluenceApi } from './api'
import { unavailableDetail } from './links'
import { AtlassianProvider } from './Provider'
import { confluenceAvailable, jiraAvailable, useAtlassianStatus, useAtlassianUi, usePrefs } from './state'

/** Explain why Confluence commands cannot run yet. */
function requireConfluence(): boolean {
  if (confluenceAvailable()) return true
  void explainUnavailable()
  return false
}

/**
 * Toast why Confluence is not available. Where no project links Atlassian, nothing asked for
 * its status yet: ask now, since the answer names the config.toml this server reads.
 */
async function explainUnavailable() {
  const s = useAtlassianStatus.getState()
  const status = s.status ?? (s.errorMessage ? null : await confluenceApi.status(useUi.getState().projectId).catch(() => null))
  toast('warning', 'Confluence is not available', { detail: unavailableDetail(s.errorMessage, status, configFileHint()), timeout: 9000 })
}

const feature: FeatureModule = {
  id: 'atlassian',
  panels: {
    confluence: { component: ConfluencePanel, icon: ConfluenceIcon },
    jira: { component: JiraPanel, icon: JiraIcon },
    'jira.board': { component: BoardPanel, icon: Kanban },
  },
  toolWindows: [
    {
      id: 'confluence',
      title: 'Confluence',
      icon: ConfluenceIcon,
      side: 'right',
      order: 20,
      component: ConfluenceToolWindow,
      // Hidden until Atlassian is configured (global [atlassian] site or the project's
      // [links.confluence]); a configured-but-broken setup shows setup help inside.
      when: (p) => !!p?.hasConfluence || confluenceAvailable(),
    },
    {
      id: 'jira',
      title: 'Jira',
      icon: JiraIcon,
      side: 'right',
      order: 25,
      component: JiraToolWindow,
      when: (p) => jiraAvailable() || !!p?.hasJira,
    },
  ],
  commands: () => [
    {
      id: 'confluence.search',
      title: 'Search Confluence',
      group: 'Confluence',
      icon: FileSearch,
      keywords: ['wiki', 'docs', 'cql', 'atlassian'],
      run: () => {
        if (!requireConfluence()) return
        showToolWindow('confluence', 'right')
        useAtlassianUi.getState().requestSearchFocus()
      },
    },
    {
      id: 'confluence.open',
      title: 'Open Confluence page (ID or URL)…',
      group: 'Confluence',
      icon: FileText,
      keywords: ['wiki', 'docs', 'link'],
      run: () => {
        if (requireConfluence()) void promptOpenPage()
      },
    },
    {
      id: 'confluence.new',
      title: 'New Confluence page…',
      group: 'Confluence',
      icon: FilePlus,
      keywords: ['wiki', 'docs', 'create'],
      run: () => {
        if (requireConfluence()) useAtlassianUi.getState().openNewPage({})
      },
    },
    {
      id: 'jira.search',
      title: 'Jira: search issues',
      group: 'Jira',
      icon: ListTodo,
      keywords: ['jql', 'issues', 'tickets'],
      when: () => jiraAvailable(),
      run: () => {
        showToolWindow('jira', 'right')
        useAtlassianUi.getState().requestJqlFocus()
      },
    },
    {
      id: 'jira.open',
      title: 'Jira: open issue (key or URL)…',
      group: 'Jira',
      icon: BookOpen,
      when: () => jiraAvailable(),
      run: () => void promptOpenIssue(),
    },
    {
      id: 'jira.boards',
      title: 'Jira: boards',
      group: 'Jira',
      icon: Kanban,
      keywords: ['sprint', 'kanban', 'scrum', 'agile', 'backlog'],
      when: () => jiraAvailable(),
      run: () => {
        usePrefs.getState().setJiraMode('boards')
        showToolWindow('jira', 'right')
      },
    },
    {
      id: 'jira.create',
      title: 'Jira: create issue…',
      group: 'Jira',
      icon: SquarePlus,
      when: () => jiraAvailable(),
      run: () => useAtlassianUi.getState().openCreateIssue({}),
    },
  ],
  mobileTabs: [{ id: 'docs', title: 'Docs', icon: ConfluenceIcon, order: 50, component: MobileDocs, when: (p) => !!p?.hasConfluence || confluenceAvailable() }],
  providers: [AtlassianProvider],
}

export default feature
