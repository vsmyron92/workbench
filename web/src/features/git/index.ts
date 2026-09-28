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
import { showToolWindow, toast, toastError } from '@/shell/actions'
import type { Command, FeatureModule } from '@/shell/types'
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
import { useDrafts, useGitPrefs, useGitUi } from './store'
import { BranchTopbarWidget, ChangesBadge, CommitBadge, GitStatusbarWidget } from './Widgets'

const commands = ({ projectId: pid }: { projectId: string | null }): Command[] => {
  if (!pid) return []
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
    { id: 'git.update', title: 'Update Project…', group: 'Git', shortcut: 'mod+t', icon: ArrowDownToLine, keywords: ['pull', 'git pull'], run: () => updateProject(pid) },
    { id: 'git.push', title: 'Push…', group: 'Git', shortcut: 'mod+shift+k', icon: ArrowUpFromLine, keywords: ['git push'], run: () => openPush(pid) },
    { id: 'git.fetch', title: 'Fetch', group: 'Git', icon: CloudDownload, keywords: ['git fetch', 'remote'], run: () => fetchAll(pid) },
    { id: 'git.log', title: 'Show Git Log', group: 'Git', shortcut: 'alt+9', icon: GitGraph, keywords: ['history', 'graph'], run: () => openGitLog(pid) },
    { id: 'git.logPanel', title: 'Open Git Log in Editor Area', group: 'Git', icon: GitGraph, keywords: ['history'], run: () => openGitLog(pid, { panel: true }) },
    { id: 'git.newBranch', title: 'New Branch…', group: 'Git', icon: GitBranchPlus, keywords: ['create branch'], run: () => newBranch(pid) },
    {
      id: 'git.branches',
      title: 'Branches…',
      group: 'Git',
      icon: GitBranch,
      keywords: ['checkout', 'switch branch', 'merge', 'rebase'],
      run: () => {
        const el = document.querySelector('.git-branch-btn') as HTMLElement | null
        useGitUi.getState().openPopover(pid, el?.getBoundingClientRect() ?? null, 'topbar')
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
        useGitUi.getState().openPopover(pid, el?.getBoundingClientRect() ?? null, 'topbar')
      },
    },
    { id: 'git.checkoutRevision', title: 'Checkout Tag or Revision…', group: 'Git', icon: Tag, run: () => checkoutRevision(pid) },
    { id: 'git.stash', title: 'Stash Changes…', group: 'Git', icon: Archive, keywords: ['git stash'], run: () => useGitUi.getState().openDialog({ kind: 'stash', projectId: pid }) },
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
          const st = await gitApi.status(pid)
          const paths = st.files.filter((f) => f.index !== '!' && !f.conflict).map((f) => f.path)
          if (!paths.length) toast('info', 'No local changes to shelve')
          else shelveChanges(pid, paths, { name: '' })
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
    { id: 'git.newChangelist', title: 'New Changelist…', group: 'Git', icon: ListPlus, keywords: ['changelist'], run: () => editChangelist(pid) },
    {
      id: 'git.rebaseInteractive',
      title: 'Interactively Rebase onto Branch…',
      group: 'Git',
      icon: GitPullRequestArrow,
      keywords: ['rebase -i', 'squash', 'reword', 'fixup'],
      run: async () => {
        const st = await gitApi.status(pid).catch(() => null)
        rebaseOntoInteractively(pid, st?.branch ?? null)
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
          const st = await gitApi.status(pid)
          if (!st.head) return toast('info', 'There is no commit yet')
          const { message } = await gitApi.lastMessage(pid)
          await undoCommit(pid, st.head, message.split('\n')[0])
        } catch (e) {
          toastError(e)
        }
      },
    },
    { id: 'git.bisectStart', title: 'Bisect: Start…', group: 'Git', icon: Crosshair, keywords: ['bisect', 'find bad commit'], run: () => startBisect(pid) },
    { id: 'git.bisectGood', title: 'Bisect: Mark Good', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void markBisect(pid, 'good') },
    { id: 'git.bisectBad', title: 'Bisect: Mark Bad', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void markBisect(pid, 'bad') },
    { id: 'git.bisectSkip', title: 'Bisect: Skip', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void markBisect(pid, 'skip') },
    { id: 'git.bisectReset', title: 'Bisect: Reset', group: 'Git', icon: Crosshair, keywords: ['bisect'], run: () => void resetBisect(pid) },
    { id: 'git.aiMessage', title: 'Ask Agent for a Commit Message', group: 'Git', icon: Sparkles, keywords: ['ai', 'claude'], run: () => void askCommitMessage(pid) },
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
    { id: 'commit', title: 'Commit', icon: GitCommitHorizontal, side: 'left', order: 20, component: CommitToolWindow, badge: CommitBadge },
    { id: 'gitlog', title: 'Git Log', icon: GitGraph, side: 'bottom', order: 20, component: GitLogToolWindow },
  ],
  commands,
  topbar: [BranchTopbarWidget],
  statusbar: [GitStatusbarWidget],
  mobileTabs: [{ id: 'git', title: 'Git', icon: GitBranch, order: 20, component: MobileGit, badge: ChangesBadge }],
  providers: [GitProvider],
}

export default feature
