// Imperative git actions shared by the commit window, log, branches popover,
// panels and commands: they call the API, confirm what is destructive and
// report the outcome in toasts.

import { api, ApiError } from '@/api/client'
import { closePanel, confirmDialog, isPanelOpen, openPanel, promptDialog, showToolWindow, toast, toastError } from '@/shell/actions'
import { askAgent } from '@/shell/agentBridge'
import { gitApi, gitUrl } from './api'
import { refsOf } from './lineSelection'
import { commitPanelId, conflictPanelId, diffPanelId, gitlogPanelId, shortSha, splitPath } from './logic'
import { useDrafts, useGitPrefs, useGitUi, useOps } from './store'
import type { BisectState, Changelist, DiffMode, OpOutcome, RebaseAction, RebaseEntry, ShelfMeta, UnshelveResult } from './types'

// ---------------------------------------------------------------- opening panels

let previewDiffId: string | null = null

/**
 * Open a diff panel. `preview` (single click in the changes list) reuses one
 * preview tab, like CLion: the previous preview closes when another opens.
 */
export function openDiff(
  pid: string,
  path: string,
  mode: DiffMode,
  o: { sha?: string; base?: string; head?: string; oldPath?: string; preview?: boolean; focus?: boolean; label?: string } = {},
) {
  const id = diffPanelId(pid, mode, path, o)
  const suffix = o.label
    ? ` (${o.label})`
    : mode === 'staged' ? ' (staged)' : mode === 'commit' && o.sha ? ` @ ${shortSha(o.sha)}` : mode === 'compare' ? ` (${o.base}..${o.head || 'working'})` : ''
  const params: Record<string, unknown> = { projectId: pid, path, mode }
  for (const k of ['sha', 'base', 'head', 'oldPath'] as const) if (o[k]) params[k] = o[k]
  const wasOpen = isPanelOpen(id)
  if (o.preview && previewDiffId && previewDiffId !== id && isPanelOpen(previewDiffId)) closePanel(previewDiffId)
  openPanel({ kind: 'diff', id, title: `${splitPath(path).name}${suffix}`, params, focus: o.focus })
  if (o.preview) {
    // A tab that was already open as a normal tab stays one.
    previewDiffId = wasOpen && previewDiffId !== id ? null : id
  } else if (previewDiffId === id) {
    previewDiffId = null // promoted to a normal tab
  }
}

export function openCommit(pid: string, sha: string) {
  openPanel({ kind: 'commit', id: commitPanelId(pid, sha), title: `Commit ${shortSha(sha)}`, params: { projectId: pid, sha } })
}

export function openConflict(pid: string, path: string) {
  openPanel({ kind: 'conflict', id: conflictPanelId(pid, path), title: `Merge: ${splitPath(path).name}`, params: { projectId: pid, path } })
}

export function openFile(pid: string, path: string, line?: number) {
  const params: Record<string, unknown> = { projectId: pid, path }
  if (line) params.line = line
  openPanel({ kind: 'editor', id: `editor:${pid}:${path}`, title: splitPath(path).name, params })
}

/** `lines` + `worktreeLines`: an editor selection (working-tree line numbers; the server maps them onto the logged revision). */
export function openGitLog(pid: string, o: { path?: string; ref?: string; panel?: boolean; lines?: [number, number]; worktreeLines?: boolean } = {}) {
  if (!o.panel && !o.path && !o.ref) {
    showToolWindow('gitlog')
    return
  }
  const params: Record<string, unknown> = { projectId: pid }
  if (o.path) params.path = o.path
  if (o.ref) params.ref = o.ref
  if (o.lines) params.lines = `${o.lines[0]},${o.lines[1]}`
  if (o.lines && o.worktreeLines) params.worktreeLines = true
  const title = o.path
    ? o.lines
      ? `History: ${splitPath(o.path).name}:${o.lines[0]}${o.lines[1] !== o.lines[0] ? `-${o.lines[1]}` : ''}`
      : `History: ${splitPath(o.path).name}`
    : 'Git Log'
  openPanel({ kind: 'gitlog', id: gitlogPanelId(pid), title, params })
}

// ---------------------------------------------------------------- helpers

export async function copyText(text: string) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
    } else {
      // Remote access over plain http has no async clipboard.
      const ta = document.createElement('textarea')
      ta.value = text
      ta.style.position = 'fixed'
      ta.style.opacity = '0'
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      ta.remove()
    }
    toast('success', `Copied ${text.length > 40 ? text.slice(0, 40) + '…' : text}`, { timeout: 2000 })
  } catch (e) {
    toastError(e, 'Copy failed')
  }
}

/**
 * Start resolving: show the Commit window (its Conflicts section lists every
 * file) and open the merge tool on the first conflicted file.
 */
export async function resolveConflicts(pid: string) {
  showToolWindow('commit')
  try {
    const st = await gitApi.status(pid)
    const first = st.files.find((f) => f.conflict)
    if (first) openConflict(pid, first.path)
    else toast('info', 'No conflicts left')
  } catch (e) {
    toastError(e, 'Could not read the git status')
  }
}

/** Report a merge/rebase/cherry-pick/stash outcome. */
export function reportOutcome(pid: string, o: OpOutcome) {
  if (o.ok) toast('success', o.message)
  else if (o.conflicts)
    toast('warning', o.message, {
      timeout: 0,
      action: { label: 'Resolve conflicts', run: () => void resolveConflicts(pid) },
    })
  else toast('error', o.message)
}

async function outcome(pid: string, path: string, body: unknown, what: string) {
  try {
    const o = await gitApi.outcome(pid, path, body)
    reportOutcome(pid, o)
    return o
  } catch (e) {
    toastError(e, what)
    return null
  }
}

// ---------------------------------------------------------------- remote operations

function newOpId() {
  return `ui-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
}

/** Start fetch/pull/push/rebase; progress arrives as `git.op` events (see GitProvider). */
export async function runRemoteOp(
  pid: string,
  op: 'fetch' | 'pull' | 'push' | 'delete-remote-branch' | 'rebase',
  body: Record<string, unknown>,
  title: string,
): Promise<boolean> {
  const opId = newOpId()
  useOps.getState().start({ opId, projectId: pid, op, title })
  const path = op === 'delete-remote-branch' ? 'remote-branches/delete' : op === 'rebase' ? 'rebase/interactive' : op
  try {
    await gitApi.post(pid, path, { ...body, opId })
    return true
  } catch (e) {
    useOps.getState().fail(opId, e instanceof Error ? e.message : String(e))
    return false
  }
}

export const fetchAll = (pid: string) => runRemoteOp(pid, 'fetch', {}, 'Fetch')
export const updateProject = (pid: string, rebase?: boolean) =>
  runRemoteOp(pid, 'pull', { autostash: true, rebase }, rebase === undefined ? 'Update project' : rebase ? 'Update (rebase)' : 'Update (merge)')
export const openPush = (pid: string) => useGitUi.getState().openDialog({ kind: 'push', projectId: pid })

// ---------------------------------------------------------------- branches

export async function checkout(pid: string, body: Record<string, unknown>, label: string) {
  try {
    const o = await gitApi.outcome(pid, 'checkout', body)
    if (o.ok) {
      toast('success', `Checked out ${label}`)
      return
    }
    if (!o.dirty) {
      reportOutcome(pid, o)
      return
    }
    const smart = await confirmDialog({
      title: 'Local changes would be overwritten',
      message: `${o.message}\n\nSmart checkout stashes your changes, checks out ${label}, and puts them back.`,
      confirmLabel: 'Smart Checkout',
    })
    if (!smart) return
    const o2 = await gitApi.outcome(pid, 'checkout', { ...body, smart: true })
    if (o2.ok) toast('success', `Checked out ${label}; local changes restored`)
    else reportOutcome(pid, o2)
  } catch (e) {
    toastError(e, 'Checkout failed')
  }
}

export const checkoutBranch = (pid: string, name: string) => checkout(pid, { ref: name }, name)

/** Check out a remote branch as a new local tracking branch (or the existing local one). */
export function checkoutRemote(pid: string, remoteName: string, branch: string, localExists: boolean) {
  if (localExists) return checkout(pid, { ref: branch }, branch)
  return checkout(pid, { create: branch, startPoint: remoteName, track: true }, branch)
}

export async function checkoutRevision(pid: string, rev?: string) {
  const r = rev ?? (await promptDialog({ title: 'Checkout tag or revision', label: 'Tag, commit hash or revision expression', placeholder: 'v1.2.0 or 1a2b3c4' }))
  if (!r?.trim()) return
  const ok = await confirmDialog({
    title: `Check out ${r.trim()}?`,
    message: 'HEAD will be detached: new commits will not belong to any branch until you create one.',
    confirmLabel: 'Checkout',
  })
  if (ok) await checkout(pid, { ref: r.trim(), detach: true }, r.trim())
}

export function newBranch(pid: string, startPoint?: string, startLabel?: string) {
  useGitUi.getState().openDialog({ kind: 'newBranch', projectId: pid, startPoint, startLabel })
}

export async function renameBranch(pid: string, name: string) {
  const next = await promptDialog({ title: `Rename branch ${name}`, label: 'New name', initial: name, confirmLabel: 'Rename' })
  if (!next?.trim() || next.trim() === name) return
  try {
    await gitApi.post(pid, 'branches/rename', { old: name, new: next.trim() })
    toast('success', `Renamed ${name} to ${next.trim()}`)
  } catch (e) {
    toastError(e, 'Rename failed')
  }
}

export async function deleteBranch(pid: string, name: string) {
  const ok = await confirmDialog({ title: `Delete branch ${name}?`, confirmLabel: 'Delete', danger: true })
  if (!ok) return
  try {
    const r = await gitApi.post<{ ok: boolean; notMerged?: boolean; message?: string }>(pid, 'branches/delete', { name })
    if (r.ok) {
      toast('success', `Deleted branch ${name}`)
      return
    }
    const force = await confirmDialog({
      title: `${name} is not fully merged`,
      message: 'Its commits are not on the current branch or its upstream. Delete it anyway? The commits stay reachable through the reflog for a while.',
      confirmLabel: 'Force Delete',
      danger: true,
    })
    if (!force) return
    await gitApi.post(pid, 'branches/delete', { name, force: true })
    toast('success', `Deleted branch ${name}`)
  } catch (e) {
    toastError(e, 'Delete failed')
  }
}

export async function deleteRemoteBranch(pid: string, remote: string, branch: string) {
  const ok = await confirmDialog({
    title: `Delete ${remote}/${branch} on the remote?`,
    message: `This runs git push ${remote} --delete ${branch}. Others lose the branch too.`,
    confirmLabel: 'Delete Remote Branch',
    danger: true,
    typed: branch,
  })
  if (ok) await runRemoteOp(pid, 'delete-remote-branch', { remote, branch }, `Delete ${remote}/${branch}`)
}

export async function mergeInto(pid: string, ref: string, current: string | null) {
  const ok = await confirmDialog({ title: `Merge ${ref} into ${current ?? 'HEAD'}?`, confirmLabel: 'Merge' })
  if (ok) await outcome(pid, 'merge', { ref }, 'Merge failed')
}

export async function rebaseOnto(pid: string, onto: string, current: string | null) {
  const ok = await confirmDialog({
    title: `Rebase ${current ?? 'HEAD'} onto ${onto}?`,
    message: 'Your commits are replayed on top of it (rewrites them). Local changes are stashed and restored.',
    confirmLabel: 'Rebase',
  })
  if (ok) await outcome(pid, 'rebase', { onto, autostash: true }, 'Rebase failed')
}

export function compareWithCurrent(pid: string, other: string, current: string | null) {
  useGitUi.getState().openDialog({ kind: 'compare', projectId: pid, base: current ?? 'HEAD', head: other })
}

export async function sequencer(pid: string, action: 'continue' | 'abort' | 'skip', what: string) {
  if (action === 'abort') {
    const ok = await confirmDialog({ title: `Abort ${what.toLowerCase()}?`, message: 'The branch goes back to where it was before.', confirmLabel: 'Abort', danger: true })
    if (!ok) return
  }
  await outcome(pid, action, {}, `${action} failed`)
}

// ---------------------------------------------------------------- commits

export async function cherryPick(pid: string, sha: string, subject: string) {
  const ok = await confirmDialog({ title: 'Cherry-pick', message: `Apply ${shortSha(sha)} “${subject}” on top of the current branch?`, confirmLabel: 'Cherry-pick' })
  if (ok) await outcome(pid, 'cherry-pick', { shas: [sha] }, 'Cherry-pick failed')
}

export async function revertCommit(pid: string, sha: string, subject: string) {
  const ok = await confirmDialog({ title: 'Revert commit', message: `Create a commit that undoes ${shortSha(sha)} “${subject}”?`, confirmLabel: 'Revert' })
  if (ok) await outcome(pid, 'revert', { shas: [sha] }, 'Revert failed')
}

export async function createTag(pid: string, sha: string) {
  const name = await promptDialog({ title: `New tag at ${shortSha(sha)}`, label: 'Tag name', placeholder: 'v1.0.0', confirmLabel: 'Create Tag' })
  if (!name?.trim()) return
  try {
    await gitApi.post(pid, 'tags', { name: name.trim(), ref: sha })
    toast('success', `Tagged ${shortSha(sha)} as ${name.trim()}`)
  } catch (e) {
    toastError(e, 'Tag failed')
  }
}

export async function deleteTag(pid: string, name: string) {
  const ok = await confirmDialog({ title: `Delete tag ${name}?`, message: 'Deletes the local tag only.', confirmLabel: 'Delete', danger: true })
  if (!ok) return
  try {
    await gitApi.post(pid, 'tags/delete', { name })
    toast('success', `Deleted tag ${name}`)
  } catch (e) {
    toastError(e, 'Delete failed')
  }
}

// ---------------------------------------------------------------- working tree

export async function stagePaths(pid: string, paths: string[]) {
  try {
    const r = await gitApi.post<{ ok: boolean; skipped?: string[] }>(pid, 'stage', { paths })
    if (r.skipped?.length)
      toast('warning', `Not staged: ${r.skipped.join(', ')} (submodule). Changes inside a submodule are committed in the submodule itself.`)
  } catch (e) {
    toastError(e, 'Stage failed')
  }
}

export async function unstagePaths(pid: string, paths: string[]) {
  try {
    await gitApi.post(pid, 'unstage', { paths })
  } catch (e) {
    toastError(e, 'Unstage failed')
  }
}

/** CLion's Rollback: confirm, then restore (tracked) or move to the trash (untracked). */
export async function rollback(pid: string, paths: string[], scope: 'all' | 'worktree') {
  if (!paths.length) return
  const list = paths.slice(0, 8).join('\n') + (paths.length > 8 ? `\n… and ${paths.length - 8} more` : '')
  const ok = await confirmDialog({
    title: paths.length === 1 ? `Roll back ${splitPath(paths[0]).name}?` : `Roll back ${paths.length} files?`,
    message: `${scope === 'worktree' ? 'Unstaged changes are discarded' : 'Staged and unstaged changes are discarded'}; unversioned files go to the trash.\n\n${list}`,
    confirmLabel: 'Rollback',
    danger: true,
  })
  if (!ok) return
  try {
    const r = await gitApi.post<{ restored: number; trashed: string[]; trashLocation: string | null; skipped?: string[] }>(pid, 'discard', { paths, scope })
    const skipped = r.skipped ?? []
    const done = paths.length - skipped.length
    toast('success', `Rolled back ${done} file${done === 1 ? '' : 's'}${r.trashed.length ? ` (${r.trashed.length} moved to the trash)` : ''}`)
    if (skipped.length)
      toast('warning', `Not rolled back: ${skipped.join(', ')} (submodule). Changes inside a submodule are reset in the submodule itself.`)
  } catch (e) {
    toastError(e, 'Rollback failed')
  }
}

// ---------------------------------------------------------------- agents

export function askCommitMessage(pid: string) {
  return askAgent({
    projectId: pid,
    prompt:
      'Write a commit message for the currently staged changes in this repository (read them with `git diff --cached`; ' +
      'if nothing is staged, describe the unstaged changes from `git diff`). Follow the repository\'s existing commit ' +
      'style (check `git log --oneline -15`). Then call the `workbench_set_commit_message` MCP tool with the full ' +
      'message. Do not run git commit and do not change any files.',
  })
}

export function askReview(pid: string, path: string, staged: boolean) {
  return askAgent({
    projectId: pid,
    prompt: `Review my uncommitted changes in ${path} (\`git diff ${staged ? '--cached ' : ''}-- ${path}\`). Point out bugs, risky or unfinished changes and anything that does not match the surrounding code. Do not modify files.`,
  })
}

export function askExplain(pid: string, sha: string, subject: string) {
  return askAgent({
    projectId: pid,
    prompt: `Explain commit ${sha} (“${subject}”): what it changes and why, and anything risky. Use \`git show ${sha}\`. Do not modify files.`,
  })
}

// ---------------------------------------------------------------- interactive rebase, undo

export function openRebase(pid: string, o: { from?: string; onto?: string; focus?: string; preset?: RebaseAction } = {}) {
  useGitUi.getState().openDialog({ kind: 'rebase', projectId: pid, ...o })
}

/** Pick a branch, then plan an interactive rebase of the current branch onto it. */
export function rebaseOntoInteractively(pid: string, current: string | null) {
  pickBranch(pid, `Interactively rebase ${current ?? 'HEAD'} onto…`, (ref) => openRebase(pid, { onto: ref }), 'Plan Rebase')
}

/** Run a prepared plan. The dialog already asked about pushed commits. */
export function runInteractiveRebase(
  pid: string,
  body: { from?: string; onto?: string; head: string; entries: RebaseEntry[]; autostash: boolean; confirmPushed: boolean },
  title: string,
) {
  return runRemoteOp(pid, 'rebase', body, title)
}

/** CLion's Undo Commit: the last (unpushed) commit goes, its changes stay staged and its message returns to the commit box. */
export async function undoCommit(pid: string, sha: string, subject: string) {
  const ok = await confirmDialog({
    title: 'Undo commit?',
    message: `${shortSha(sha)} “${subject}” is removed from the branch. Its changes stay staged and its message goes back into the commit box.`,
    confirmLabel: 'Undo Commit',
  })
  if (!ok) return
  try {
    const r = await gitApi.post<{ ok: boolean; message: string }>(pid, 'undo-commit', { sha })
    useDrafts.getState().update(pid, { message: r.message, amend: false, amendLoaded: undefined })
    useGitPrefs.getState().setTab('changes')
    showToolWindow('commit')
    toast('success', `Undid ${shortSha(sha)}; its changes are staged`)
  } catch (e) {
    toastError(e, 'Undo commit failed')
  }
}

// ---------------------------------------------------------------- branch picker, compare

export function pickBranch(pid: string, title: string, onPick: (ref: string) => void, confirmLabel?: string) {
  useGitUi.getState().openDialog({ kind: 'pickBranch', projectId: pid, title, onPick, confirmLabel })
}

/** Compare a file with its version on another branch (working tree on the right). */
export function compareWithBranch(pid: string, path: string) {
  pickBranch(pid, `Compare ${splitPath(path).name} with branch`, (ref) => openDiff(pid, path, 'compare', { base: ref, head: '' }), 'Compare')
}

// ---------------------------------------------------------------- lines

/** Stage / unstage / roll back selected lines of a working or staged diff. */
export async function applyLines(pid: string, op: 'stage' | 'unstage' | 'discard', path: string, fingerprint: string, keys: Iterable<string>) {
  const lines = refsOf(keys)
  if (!lines.length) return false
  if (op === 'discard') {
    const ok = await confirmDialog({
      title: `Roll back ${lines.length} line${lines.length === 1 ? '' : 's'}?`,
      message: `The selected changes of ${splitPath(path).name} go back to the staged version. The rest of the file stays.`,
      confirmLabel: 'Rollback',
      danger: true,
    })
    if (!ok) return false
  }
  try {
    await gitApi.post(pid, `${op}-lines`, { path, fingerprint, lines })
    return true
  } catch (e) {
    if (e instanceof ApiError && e.status === 409) toast('warning', e.message)
    else toastError(e, `${op === 'stage' ? 'Stage' : op === 'unstage' ? 'Unstage' : 'Rollback'} failed`)
    return false
  }
}

// ---------------------------------------------------------------- bisect

export function startBisect(pid: string, o: { good?: string; bad?: string } = {}) {
  useGitUi.getState().openDialog({ kind: 'bisectStart', projectId: pid, ...o })
}

export async function markBisect(pid: string, verdict: 'good' | 'bad' | 'skip', rev?: string) {
  try {
    const r = await gitApi.post<{ message: string; state: BisectState }>(pid, 'bisect/mark', { verdict, rev })
    if (r.state.result) toast('success', r.message || `First bad commit: ${shortSha(r.state.result)}`, { timeout: 0, action: { label: 'Show commit', run: () => openCommit(pid, r.state.result!) } })
    else toast('info', r.message || 'Marked')
  } catch (e) {
    toastError(e, 'Bisect failed')
  }
}

export async function resetBisect(pid: string) {
  try {
    await gitApi.post(pid, 'bisect/reset')
    toast('success', 'Bisect reset: back where you started')
  } catch (e) {
    toastError(e, 'Bisect reset failed')
  }
}

// ---------------------------------------------------------------- changelists

export function editChangelist(pid: string, list?: Changelist, paths?: string[]) {
  useGitUi.getState().openDialog({ kind: 'changelist', projectId: pid, list, paths })
}

export async function moveToChangelist(pid: string, paths: string[], to: string) {
  if (!paths.length) return
  try {
    await gitApi.post(pid, 'changelists/move', { paths, to })
  } catch (e) {
    toastError(e, 'Move failed')
  }
}

export async function setActiveChangelist(pid: string, id: string) {
  try {
    await api.patch(gitUrl(pid, `changelists/${encodeURIComponent(id)}`), { active: true })
  } catch (e) {
    toastError(e)
  }
}

export async function deleteChangelist(pid: string, list: Changelist, activeName: string) {
  const ok = await confirmDialog({
    title: `Delete changelist “${list.name}”?`,
    message: list.files.length
      ? `Its ${list.files.length} file${list.files.length === 1 ? '' : 's'} move${list.files.length === 1 ? 's' : ''} to ${list.active ? 'the first remaining list' : `“${activeName}”`}; no change is lost.`
      : undefined,
    confirmLabel: 'Delete',
    danger: true,
  })
  if (!ok) return
  try {
    await api.del(gitUrl(pid, `changelists/${encodeURIComponent(list.id)}`))
  } catch (e) {
    toastError(e, 'Delete failed')
  }
}

// ---------------------------------------------------------------- shelf

export function shelveChanges(pid: string, paths: string[], o: { changelist?: string; name?: string } = {}) {
  useGitUi.getState().openDialog({ kind: 'shelve', projectId: pid, paths, ...o })
}

export async function unshelve(pid: string, shelf: ShelfMeta, o: { paths?: string[]; remove?: boolean; changelist?: string } = {}) {
  try {
    const r = await gitApi.post<UnshelveResult>(pid, `shelf/${encodeURIComponent(shelf.id)}/unshelve`, o)
    if (r.ok) toast('success', r.message)
    else
      toast('warning', r.message, {
        timeout: 0,
        action: { label: 'Resolve conflicts', run: () => void resolveConflicts(pid) },
      })
    return r
  } catch (e) {
    toastError(e, 'Unshelve failed')
    return null
  }
}

export async function renameShelf(pid: string, shelf: ShelfMeta) {
  const name = await promptDialog({ title: 'Rename shelved changes', label: 'Name', initial: shelf.name, confirmLabel: 'Rename' })
  if (!name?.trim() || name.trim() === shelf.name) return
  try {
    await api.patch(gitUrl(pid, `shelf/${encodeURIComponent(shelf.id)}`), { name: name.trim() })
  } catch (e) {
    toastError(e, 'Rename failed')
  }
}

export async function deleteShelf(pid: string, shelf: ShelfMeta) {
  const ok = await confirmDialog({
    title: `Delete “${shelf.name}”?`,
    message: `The ${shelf.files.length} shelved file${shelf.files.length === 1 ? '' : 's'} are deleted for good.`,
    confirmLabel: 'Delete',
    danger: true,
  })
  if (!ok) return
  try {
    await api.del(gitUrl(pid, `shelf/${encodeURIComponent(shelf.id)}`))
    toast('success', 'Shelf deleted')
  } catch (e) {
    toastError(e, 'Delete failed')
  }
}
