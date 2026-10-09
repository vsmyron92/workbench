// Feature slice: git (CLion-style version control). Owned by the git slice —
// see docs/ARCHITECTURE.md. Panels: diff, commit, gitlog, conflict. Tool
// windows: commit (left: Changes / Stash / Shelf), gitlog (bottom). Events:
// git.changed, fs.changed, git.op, git.commitMessage, git.changelists, git.shelf.

import {
  Archive,
  ArrowDownToLine,
  ArrowUpFromLine,
  CloudDownload,
  Crosshair,
  FileDiff,
  FolderGit2,
  GitBranch,
  GitBranchPlus,
  GitCommitHorizontal,
  GitGraph,
  GitMerge,
  GitPullRequestArrow,
  ListPlus,
  PackageOpen,
  PackagePlus,
  Sparkles,
  Tag,
  Undo,
} from 'lucide-react'
import type { ProjectSummary } from '@/api/types'
import { activeScope, resolveRepo, setActiveRepo, useActiveRepoStore } from '@/api/repos'
import { withGitScope } from '@/api/useRepos'
import { showToolWindow, toast, toastError } from '@/shell/actions'
import type { Command, CommandContext, FeatureModule } from '@/shell/types'
import { gitApi } from './api'
import {
  askCommitMessage,
  checkoutRevision,
  editChangelist,
  fetchAll,
  markBisect,
  newBranch,
  openGitLog,
  openPush,
  rebaseOntoInteractively,
  resetBisect,
  shelveChanges,
  startBisect,
  undoCommit,
  updateProject,
} from './actions'
import { CommitPanel } from './CommitDetails'
import { CommitToolWindow } from './CommitWindow'
import { ConflictPanel } from './ConflictPanel'
import { DiffPanel } from './DiffPanel'
import { GitProvider } from './GitProvider'
import { GitLogPanel, GitLogToolWindow } from './LogView'
import { MobileGit } from './MobileGit'
import { openRepoSwitcher, RepoTopbarWidget } from './RepoSwitcher'
import { useDrafts, useGitPrefs, useGitUi } from './store'
import { BranchTopbarWidget, ChangesBadge, CommitBadge, GitStatusbarWidget } from './Widgets'

const commands = ({ projectId, project }: CommandContext): Command[] => {
  if (!projectId) return []
  // The repository the git views are on when the command runs, not when the palette was built.
  const scope = () => activeScope(projectId)
  return [
    {
      id: 'git.commit',
      title: 'Commit…',
      group: 'Git',
      shortcut: 'alt+0',
      icon: GitCommitHorizontal,
      keywords: ['vcs', 'changes'],
      run: () => {
        showToolWindow('commit')
        useDrafts.getState().focus()
      },
    },
    { id: 'git.update', title: 'Update Project…', group: 'Git', shortcut: 'mod+t', icon: ArrowDownToLine, keywords: ['pull', 'git pull'], run: () => updateProject(scope()) },
    { id: 'git.push', title: 'Push…', group: 'Git', shortcut: 'mod+shift+k', icon: ArrowUpFromLine, keywords: ['git push'], run: () => openPush(scope()) },
    { id: 'git.fetch', title: 'Fetch', group: 'Git', icon: CloudDownload, keywords: ['git fetch', 'remote'], run: () => fetchAll(scope()) },
    { id: 'git.log', title: 'Show Git Log', group: 'Git', shortcut: 'alt+9', icon: GitGraph, keywords: ['history', 'graph'], run: () => openGitLog(scope()) },
    { id: 'git.logPanel', title: 'Open Git Log in Editor Area', group: 'Git', icon: GitGraph, keywords: ['history'], run: () => openGitLog(scope(), { panel: true }) },
    { id: 'git.newBranch', title: 'New Branch…', group: 'Git', icon: GitBranchPlus, keywords: ['create branch'], run: () => newBranch(scope()) },
    {
      id: 'git.branches',
      title: 'Branches…',
      group: 'Git',
      icon: GitBranch,
      keywords: ['checkout', 'switch branch', 'merge', 'rebase'],
      run: () => {
        const el = document.querySelector('.git-branch-btn') as HTMLElement | null
        useGitUi.getState().openPopover(scope(), el?.getBoundingClientRect() ?? null, 'topbar')
      },
    },
    {
      id: 'git.checkout',
      title: 'Checkout Branch…',
      group: 'Git',
      icon: GitBranch,
      keywords: ['switch'],
      run: () => {
        const el = document.querySelector('.git-branch-btn') as HTMLElement | null
        useGitUi.getState().openPopover(scope(), el?.getBoundingClientRect() ?? null, 'topbar')
      },
    },
    { id: 'git.checkoutRevision', title: 'Checkout Tag or Revision…', group: 'Git', icon: Tag, run: () => checkoutRevision(scope()) },
    { id: 'git.stash', title: 'Stash Changes…', group: 'Git', icon: Archive, keywords: ['git stash'], run: () => useGitUi.getState().openDialog({ kind: 'stash', projectId: scope() }) },
    {
      id: 'git.unstash',
      title: 'Unstash Changes…',
      group: 'Git',
      icon: Archive,
      keywords: ['stash pop', 'apply stash'],
      run: () => {
        useGitPrefs.getState().setTab('stash')
        showToolWindow('commit')
      },
    },
    {
      id: 'git.shelve',
      title: 'Shelve Changes…',
      group: 'Git',
      icon: PackagePlus,
      keywords: ['shelf', 'set aside'],
      run: async () => {
        try {
          const st = await gitApi.status(scope())
          const paths = st.files.filter((f) => f.index !== '!' && !f.conflict).map((f) => f.path)
          if (!paths.length) toast('info', 'No local changes to shelve')
          else shelveChanges(scope(), paths, { name: '' })
        } catch (e) {
          toastError(e)
        }
      },
    },
    {
      id: 'git.unshelve',
      title: 'Unshelve Changes…',
      group: 'Git',
      icon: PackageOpen,
      keywords: ['shelf'],
      run: () => {
        useGitPrefs.getState().setTab('shelf')
        showToolWindow('commit')
      },
    },
    { id: 'git.newChangelist', title: 'New Changelist…', group: 'Git', icon: ListPlus, keywords: ['changelist'], run: () => editChangelist(scope()) },
    {
      id: 'git.rebaseInteractive',
      title: 'Interactively Rebase onto Branch…',
      group: 'Git',
      icon: GitPullRequestArrow,
      keywords: ['rebase -i', 'squash', 'reword', 'fixup'],
      run: async () => {
        const st = await gitApi.status(scope()).catch(() => null)
        rebaseOntoInteractively(scope(), st?.branch ?? null)
      },
    },
    {
      id: 'git.undoCommit',
      title: 'Undo Last Commit…',
      group: 'Git',
      icon: Undo,
      keywords: ['reset soft', 'uncommit'],
      run: async () => {
        try {
          const st = await gitApi.status(scope())
          if (!st.head) return toast('info', 'There is no commit yet')
          const { message } = await gitApi.lastMessage(scope())
          await undoCommit(scope(), st.head, message.split('\n')[0])
        } catch (e) {
          toastError(e)
        }
      },
    },
    { id: 'git.bisectStart', title: 'Bisect: Start…', group: 'Git', icon: Crosshair, keywords: ['bisect', 'find bad commit'], run: () => startBisect(scope()) },
    { id: 'git.bisectGood', title: 'Bisect: Mark Good', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void markBisect(scope(), 'good') },
    { id: 'git.bisectBad', title: 'Bisect: Mark Bad', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void markBisect(scope(), 'bad') },
    { id: 'git.bisectSkip', title: 'Bisect: Skip', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void markBisect(scope(), 'skip') },
    { id: 'git.bisectReset', title: 'Bisect: Reset', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void resetBisect(scope()) },
    { id: 'git.aiMessage', title: 'Ask Agent for a Commit Message', group: 'Git', icon: Sparkles, keywords: ['ai', 'claude'], run: () => void askCommitMessage(scope()) },
    ...repoCommands(projectId, project),
  ]
}

/** Switch the git and CI views to another repository of the project. */
function repoCommands(projectId: string, project: ProjectSummary | null): Command[] {
  const repos = project?.repos ?? []
  if (repos.length < 2) return []
  const current = resolveRepo(repos, useActiveRepoStore.getState().active[projectId])?.id
  return [
    { id: 'git.switchRepo', title: 'Switch Repository…', group: 'Git', icon: FolderGit2, keywords: ['repo', 'repository', 'project'], run: () => openRepoSwitcher(projectId) },
    ...repos
      .filter((r) => r.id !== current)
      .map<Command>((r) => ({
        id: `git.switchRepo.${r.id}`,
        title: `Git: Switch to ${r.name}`,
        group: 'Git',
        icon: FolderGit2,
        keywords: ['repo', 'repository', r.path, r.id],
        run: () => setActiveRepo(projectId, r.id),
      })),
  ]
}

const feature: FeatureModule = {
  id: 'git',
  panels: {
    diff: { component: DiffPanel, icon: FileDiff },
    commit: { component: CommitPanel, icon: GitCommitHorizontal },
    gitlog: { component: GitLogPanel, icon: GitGraph },
    conflict: { component: ConflictPanel, icon: GitMerge },
  },
  toolWindows: [
    { id: 'commit', title: 'Commit', icon: GitCommitHorizontal, side: 'left', order: 20, component: withGitScope(CommitToolWindow), badge: withGitScope(CommitBadge) },
    { id: 'gitlog', title: 'Git Log', icon: GitGraph, side: 'bottom', order: 20, component: withGitScope(GitLogToolWindow) },
  ],
  commands,
  // The widgets and windows get the shell's project id and show its active repository.
  topbar: [RepoTopbarWidget, withGitScope(BranchTopbarWidget)],
  statusbar: [withGitScope(GitStatusbarWidget)],
  mobileTabs: [{ id: 'git', title: 'Git', icon: GitBranch, order: 20, component: MobileGit, badge: withGitScope(ChangesBadge) }],
  providers: [GitProvider],
}

export default feature
