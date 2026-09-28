// Feature slice: github. Owned by the github slice — see docs/ARCHITECTURE.md.
//
// Panels: 'pr', 'gh.run', 'gh.job' and 'gh.issue' (contract). Tool window
// 'github' (right), CI widgets in the top and status bars for projects whose
// forge is GitHub, palette commands, the phone's 'github' tab, and a provider
// that keeps views fresh from events.
//
// Only the provider (with its dialogs), the badge and the bar widgets load
// with the app; panels, the tool window and the phone tab load when first shown.

import { lazy } from 'react'
import { CircleDot, FileText, GitPullRequest, GitPullRequestCreate, Play, Workflow } from 'lucide-react'
import { api } from '@/api/client'
import { showToolWindow, toast, toastError } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { gh } from './api'
import { GithubBadge } from './Badge'
import { GitHubIcon, openPr, openRun, runTitle, useGhUi, type GhTab } from './components'
import { openOnPhone } from './mobile'
import { GithubProvider } from './providers'
import type { GithubSummary } from './types'
import { CiTopbarWidget, RunStatusItem } from './widgets'

const PrPanel = lazy(() => import('./PrPanel').then((m) => ({ default: m.PrPanel })))
const RunPanel = lazy(() => import('./RunPanel').then((m) => ({ default: m.RunPanel })))
const JobPanel = lazy(() => import('./JobPanel').then((m) => ({ default: m.JobPanel })))
const IssuePanel = lazy(() => import('./IssuePanel').then((m) => ({ default: m.IssuePanel })))
const GithubToolWindow = lazy(() => import('./ToolWindow').then((m) => ({ default: m.GithubToolWindow })))
const MobileGithub = lazy(() => import('./MobileGithub'))

async function summaryOf(pid: string): Promise<GithubSummary | null> {
  try {
    return await api.get<GithubSummary>(`${gh(pid)}/summary`)
  } catch (e) {
    toastError(e, 'GitHub')
    return null
  }
}

const feature: FeatureModule = {
  id: 'github',
  panels: {
    pr: { component: PrPanel, icon: GitPullRequest },
    'gh.run': { component: RunPanel, icon: Workflow },
    'gh.job': { component: JobPanel, icon: FileText },
    'gh.issue': { component: IssuePanel, icon: CircleDot },
  },
  toolWindows: [
    {
      id: 'github',
      title: 'GitHub',
      icon: GitHubIcon,
      side: 'right',
      order: 12,
      component: GithubToolWindow,
      badge: GithubBadge,
      when: (p) => !!p?.github,
    },
  ],
  commands: (ctx) => {
    const pid = ctx.projectId
    if (!pid || !ctx.project?.github) return []
    const show = (tab: GhTab) => {
      useGhUi.getState().setTab(tab)
      showToolWindow('github')
    }
    const withToken = async (what: string, run: () => void) => {
      const s = await summaryOf(pid)
      if (!s) return
      if (!s.auth.authenticated) toast('info', `${what} needs a GitHub token (this project is read in public, read-only mode)`)
      else run()
    }
    return [
      {
        id: 'github.openActions',
        title: 'GitHub: Open Actions',
        group: 'GitHub',
        icon: Workflow,
        keywords: ['ci', 'workflows', 'runs', 'builds', 'actions'],
        run: () => show('runs'),
      },
      {
        id: 'github.openLatestRun',
        title: 'GitHub: Open latest run of the current branch',
        group: 'GitHub',
        icon: Workflow,
        keywords: ['ci', 'build', 'status', 'actions'],
        run: async () => {
          const s = await summaryOf(pid)
          if (!s) return
          const r = s.branchRun ?? s.defaultRun
          if (r) openRun(pid, r.id, runTitle(r, r.id))
          else toast('info', `No workflow runs for ${s.branch ?? 'this branch'} yet`)
        },
      },
      {
        id: 'github.runWorkflow',
        title: 'GitHub: Run workflow…',
        group: 'GitHub',
        icon: Play,
        keywords: ['ci', 'dispatch', 'trigger', 'actions'],
        run: () => withToken('Running a workflow', () => useGhUi.getState().openRunWorkflow(pid)),
      },
      {
        id: 'github.createPr',
        title: 'GitHub: Create pull request…',
        group: 'GitHub',
        icon: GitPullRequestCreate,
        keywords: ['pr', 'pull request', 'merge request', 'mr'],
        run: () => withToken('Creating a pull request', () => useGhUi.getState().openCreatePr(pid)),
      },
      {
        id: 'github.openCurrentPr',
        title: 'GitHub: Open pull request for the current branch',
        group: 'GitHub',
        icon: GitPullRequest,
        keywords: ['pr', 'pull request', 'review'],
        run: async () => {
          const s = await summaryOf(pid)
          if (!s) return
          if (s.currentPr) openPr(pid, s.currentPr.number, s.currentPr.title)
          else
            toast('info', `No pull request for ${s.branch ?? 'this branch'}`, {
              action: s.auth.authenticated ? { label: 'Create one', run: () => useGhUi.getState().openCreatePr(pid) } : undefined,
            })
        },
      },
      {
        id: 'github.openPulls',
        title: 'GitHub: Show pull requests',
        group: 'GitHub',
        icon: GitPullRequest,
        keywords: ['pr', 'pull requests'],
        run: () => show('pulls'),
      },
      {
        id: 'github.openIssues',
        title: 'GitHub: Show issues',
        group: 'GitHub',
        icon: CircleDot,
        run: () => show('issues'),
      },
    ]
  },
  topbar: [CiTopbarWidget],
  statusbar: [RunStatusItem],
  mobileTabs: [
    {
      id: 'github',
      title: 'GitHub',
      icon: GitHubIcon,
      order: 42,
      component: MobileGithub,
      badge: GithubBadge,
      when: (p) => !!p?.github,
      // Runs, jobs, pull requests and issues open inside the tab on a phone.
      openPanel: openOnPhone,
    },
  ],
  providers: [GithubProvider],
}

export default feature
