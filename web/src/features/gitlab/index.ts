// Feature slice: gitlab. Owned by the gitlab slice — see docs/ARCHITECTURE.md.
//
// Panels: 'mr', 'pipeline', 'job' (contract) and 'gitlab.issue'. Tool window
// 'gitlab' (right), CI widgets in the top and status bars, palette commands,
// the phone's 'ci' tab, and a provider that keeps views fresh from events.

import { CircleDot, FileText, GitPullRequest, GitPullRequestCreate, Play, Workflow } from 'lucide-react'
import { api } from '@/api/client'
import { showToolWindow, toast, toastError } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { GitLabIcon } from '@/ui'
import { gl } from './api'
import { GitlabBadge } from './Badge'
import { openMr, openPipeline, useGlUi } from './components'
import { IssuePanel } from './IssuePanel'
import { JobPanel } from './JobPanel'
import { MobileCi } from './MobileCi'
import { MrPanel } from './MrPanel'
import { PipelinePanel } from './PipelinePanel'
import { GitlabProvider } from './providers'
import { GitlabToolWindow } from './ToolWindow'
import type { GitlabSummary } from './types'
import { CiTopbarWidget, PipelineStatusItem } from './widgets'

async function summaryOf(pid: string): Promise<GitlabSummary | null> {
  try {
    return await api.get<GitlabSummary>(`${gl(pid)}/summary`)
  } catch (e) {
    toastError(e, 'GitLab')
    return null
  }
}

const feature: FeatureModule = {
  id: 'gitlab',
  panels: {
    mr: { component: MrPanel, icon: GitPullRequest },
    pipeline: { component: PipelinePanel, icon: Workflow },
    job: { component: JobPanel, icon: FileText },
    'gitlab.issue': { component: IssuePanel, icon: CircleDot },
  },
  toolWindows: [
    {
      id: 'gitlab',
      title: 'GitLab',
      icon: GitLabIcon,
      side: 'right',
      order: 10,
      component: GitlabToolWindow,
      badge: GitlabBadge,
      when: (p) => !!p?.gitlab,
    },
  ],
  commands: (ctx) => {
    const pid = ctx.projectId
    if (!pid || !ctx.project?.gitlab) return []
    const show = (tab: 'pipelines' | 'mrs' | 'issues') => {
      useGlUi.getState().setTab(tab)
      showToolWindow('gitlab')
    }
    return [
      {
        id: 'gitlab.openPipelines',
        title: 'GitLab: Show pipelines',
        group: 'GitLab',
        icon: Workflow,
        keywords: ['ci', 'pipelines', 'builds'],
        run: () => show('pipelines'),
      },
      {
        id: 'gitlab.openLatestPipeline',
        title: 'GitLab: Open latest pipeline of the current branch',
        group: 'GitLab',
        icon: Workflow,
        keywords: ['ci', 'build', 'status'],
        run: async () => {
          const s = await summaryOf(pid)
          if (!s) return
          const p = s.branchPipeline ?? s.defaultPipeline
          if (p) openPipeline(pid, p.id, p.iid)
          else toast('info', `No pipelines for ${s.branch ?? 'this branch'} yet`)
        },
      },
      {
        id: 'gitlab.runPipeline',
        title: 'GitLab: Run pipeline…',
        group: 'GitLab',
        icon: Play,
        keywords: ['ci', 'trigger'],
        run: () => useGlUi.getState().openRunPipeline(pid),
      },
      {
        id: 'gitlab.createMr',
        title: 'GitLab: Create merge request…',
        group: 'GitLab',
        icon: GitPullRequestCreate,
        keywords: ['mr', 'pull request', 'pr'],
        run: () => useGlUi.getState().openCreateMr(pid),
      },
      {
        id: 'gitlab.openCurrentMr',
        title: 'GitLab: Open merge request for the current branch',
        group: 'GitLab',
        icon: GitPullRequest,
        keywords: ['mr', 'pull request', 'pr', 'review'],
        run: async () => {
          const s = await summaryOf(pid)
          if (!s) return
          if (s.currentMr) openMr(pid, s.currentMr.iid, s.currentMr.title)
          else
            toast('info', `No merge request for ${s.branch ?? 'this branch'}`, {
              action: { label: 'Create one', run: () => useGlUi.getState().openCreateMr(pid) },
            })
        },
      },
      {
        id: 'gitlab.openMrs',
        title: 'GitLab: Show merge requests',
        group: 'GitLab',
        icon: GitPullRequest,
        keywords: ['mr', 'pull requests'],
        run: () => show('mrs'),
      },
      {
        id: 'gitlab.openIssues',
        title: 'GitLab: Show issues',
        group: 'GitLab',
        icon: CircleDot,
        run: () => show('issues'),
      },
    ]
  },
  topbar: [CiTopbarWidget],
  statusbar: [PipelineStatusItem],
  mobileTabs: [{ id: 'ci', title: 'CI', icon: Workflow, order: 40, component: MobileCi, badge: GitlabBadge, when: (p) => !!p?.gitlab }],
  providers: [GitlabProvider],
}

export default feature
