// File operations behind the tree's context menu, keyboard shortcuts and
// commands: create, rename, duplicate, move, delete (to the trash), upload.

import {
  Bot,
  Clock,
  Copy,
  Download,
  ExternalLink,
  FilePlus,
  FolderPlus,
  GitCompare,
  History,
  Link2,
  Pencil,
  RefreshCw,
  SquareTerminal,
  TextSearch,
  Trash2,
  Eye,
  Code,
  Columns2,
} from 'lucide-react'
import { ApiError } from '@/api/client'
import { closePanel, confirmDialog, promptDialog, toast, toastError } from '@/shell/actions'
import type { MenuEntry } from '@/ui'
import { filesApi } from './api'
import { bufferKey, useBuffers } from './buffers'
import {
  absolutePath,
  askAboutFile,
  compareWithHead,
  compareWithPicked,
  openFile,
  openMarkdown,
  openTerminalAt,
  panelsFor,
  revealInTree,
  showHistory,
  showSearch,
} from './openers'
import { showLocalHistory } from './history/open'
import { basename, dirname, isMarkdown, isWithin, joinPath } from './paths'
import { copyText } from './text'
import { forgetDir, refreshDirs } from './treeLoader'

const MAX_UPLOAD = 200 * 1024 * 1024
const MAX_UPLOAD_FILES = 100

export interface Target {
  projectId: string
  rootAbs?: string
  /** Project-relative; `''` = the project root. */
  path: string
  isDir: boolean
}

const parentOf = (p: string) => (dirname(p) === '/' ? '' : dirname(p))

function cleanName(input: string): string | null {
  const name = input.trim().replace(/^\/+/, '').replace(/\/+$/, '')
  if (!name || name.split('/').some((s) => s === '..' || s === '.' || s === '')) return null
  return name
}

/** Close editor/preview panels of `path` (or anything under it) that have no unsaved changes. */
function closeCleanPanels(projectId: string, path: string, reopenAt?: string) {
  const buffers = useBuffers.getState().buffers
  for (const b of Object.values(buffers)) {
    if (b.projectId !== projectId || !isWithin(path, b.path)) continue
    if (b.dirty) continue
    const target = reopenAt ? reopenAt + b.path.slice(path.length) : null
    for (const panel of panelsFor('editor', projectId, b.path)) closePanel(panel.id)
    if (target) openFile({ projectId, path: target, focus: false })
  }
  if (!reopenAt) for (const panel of panelsFor('markdown', projectId, path)) closePanel(panel.id)
}

export async function newEntry(projectId: string, dir: string, kind: 'file' | 'folder') {
  const input = await promptDialog({
    title: kind === 'file' ? 'New File' : 'New Folder',
    label: `In ${dir || 'the project root'} (use / to create subfolders)`,
    placeholder: kind === 'file' ? 'name.ext' : 'folder',
    confirmLabel: 'Create',
  })
  if (input === null) return
  const name = cleanName(input)
  if (!name) {
    toast('warning', 'Invalid name')
    return
  }
  const path = joinPath(dir, name)
  try {
    await filesApi.op(projectId, kind === 'file' ? 'create' : 'mkdir', path)
  } catch (e) {
    toastError(e, `Could not create ${name}`)
    return
  }
  refreshDirs(projectId, [dir, parentOf(path)])
  revealInTree(projectId, path)
  if (kind === 'file') openFile({ projectId, path })
}

export async function renameEntry(t: Target) {
  if (!t.path) return
  const old = basename(t.path)
  const input = await promptDialog({ title: t.isDir ? 'Rename Folder' : 'Rename File', initial: old, confirmLabel: 'Rename' })
  if (input === null || input.trim() === old) return
  const name = cleanName(input)
  if (!name) {
    toast('warning', 'Invalid name')
    return
  }
  const to = joinPath(parentOf(t.path), name)
  try {
    await filesApi.op(t.projectId, 'rename', t.path, to)
  } catch (e) {
    toastError(e, `Could not rename ${old}`)
    return
  }
  closeCleanPanels(t.projectId, t.path, to)
  if (t.isDir) forgetDir(t.projectId, t.path)
  refreshDirs(t.projectId, [parentOf(t.path), parentOf(to)])
  revealInTree(t.projectId, to)
}

export async function duplicateEntry(t: Target) {
  if (!t.path) return
  const old = basename(t.path)
  const dot = old.lastIndexOf('.')
  const suggestion = !t.isDir && dot > 0 ? `${old.slice(0, dot)} copy${old.slice(dot)}` : `${old} copy`
  const input = await promptDialog({ title: `Duplicate ${old}`, initial: suggestion, confirmLabel: 'Duplicate' })
  if (input === null) return
  const name = cleanName(input)
  if (!name) return
  const to = joinPath(parentOf(t.path), name)
  try {
    await filesApi.op(t.projectId, 'copy', t.path, to)
  } catch (e) {
    toastError(e, `Could not duplicate ${old}`)
    return
  }
  refreshDirs(t.projectId, [parentOf(to)])
  revealInTree(t.projectId, to)
}

export async function moveEntry(projectId: string, path: string, toDir: string) {
  const name = basename(path)
  if (parentOf(path) === toDir || isWithin(path, toDir)) return
  const ok = await confirmDialog({ title: `Move ${name}?`, message: `Move “${path}” to “${toDir || 'the project root'}”?`, confirmLabel: 'Move' })
  if (!ok) return
  const to = joinPath(toDir, name)
  try {
    await filesApi.op(projectId, 'rename', path, to)
  } catch (e) {
    toastError(e, `Could not move ${name}`)
    return
  }
  closeCleanPanels(projectId, path, to)
  forgetDir(projectId, path)
  refreshDirs(projectId, [parentOf(path), toDir])
  revealInTree(projectId, to)
}

export async function deleteEntry(t: Target) {
  if (!t.path) return
  const name = basename(t.path)
  const dirty = Object.values(useBuffers.getState().buffers).some((b) => b.projectId === t.projectId && isWithin(t.path, b.path) && b.dirty)
  const ok = await confirmDialog({
    title: `Delete ${t.isDir ? 'folder' : 'file'} “${name}”?`,
    message: `It is moved to the trash${t.isDir ? ' with everything in it' : ''}.${dirty ? '\nAn open editor has unsaved changes to it.' : ''}`,
    confirmLabel: 'Move to Trash',
    danger: true,
  })
  if (!ok) return
  try {
    await filesApi.op(t.projectId, 'delete', t.path)
  } catch (e) {
    toastError(e, `Could not delete ${name}`)
    return
  }
  closeCleanPanels(t.projectId, t.path)
  forgetDir(t.projectId, t.path)
  refreshDirs(t.projectId, [parentOf(t.path)])
  toast('success', `Moved ${name} to the trash`)
}

/** Upload OS files (drag and drop) into `dir`, one at a time. */
export async function uploadFiles(projectId: string, dir: string, files: File[]) {
  if (!files.length) return
  if (files.length > MAX_UPLOAD_FILES) {
    toast('warning', `Drop at most ${MAX_UPLOAD_FILES} files at once`)
    return
  }
  const tooBig = files.filter((f) => f.size > MAX_UPLOAD)
  const ok: string[] = []
  const failed: string[] = []
  for (const f of files) {
    if (f.size > MAX_UPLOAD) continue
    try {
      const r = await filesApi.upload(projectId, dir, f)
      ok.push(r.name)
    } catch (e) {
      failed.push(`${f.name}: ${e instanceof ApiError ? e.message : String(e)}`)
    }
  }
  refreshDirs(projectId, [dir])
  if (ok.length) toast('success', ok.length === 1 ? `Uploaded ${ok[0]}` : `Uploaded ${ok.length} files`, { detail: dir ? `to ${dir}` : 'to the project root' })
  if (tooBig.length) toast('warning', `Skipped ${tooBig.length} file(s) over 200 MB`, { detail: tooBig.map((f) => f.name).join(', ') })
  if (failed.length) toast('error', `${failed.length} upload(s) failed`, { detail: failed.slice(0, 3).join('\n') })
}

async function copyToClipboard(text: string, what: string) {
  if (await copyText(text)) toast('success', `Copied ${what}`, { detail: text, timeout: 2500 })
  else toast('error', 'Could not copy to the clipboard')
}

export function downloadFile(projectId: string, path: string) {
  const a = document.createElement('a')
  a.href = filesApi.rawUrl(projectId, path, { download: true })
  a.download = basename(path)
  document.body.appendChild(a)
  a.click()
  a.remove()
}

/** The tree's context menu for `t`. */
export function treeMenu(t: Target): MenuEntry[] {
  const dir = t.isDir ? t.path : parentOf(t.path)
  const abs = absolutePath(t.rootAbs, t.path)
  const items: MenuEntry[] = []
  if (!t.isDir) {
    items.push({ label: 'Open', icon: ExternalLink, shortcut: 'Enter', run: () => openFile({ projectId: t.projectId, path: t.path }) })
    items.push({ label: 'Open to the Side', icon: Columns2, run: () => openFile({ projectId: t.projectId, path: t.path, side: true }) })
    if (t.projectId) {
      const pid = t.projectId
      items.push({ label: 'Compare With…', icon: GitCompare, run: () => compareWithPicked(pid, t.path) })
    }
    if (isMarkdown(t.path)) {
      items.push({ label: 'Edit Source', icon: Code, run: () => openFile({ projectId: t.projectId, path: t.path, mode: 'edit' }) })
      items.push({ label: 'Open Live Preview', icon: Eye, run: () => openMarkdown(t.projectId, t.path) })
    }
    items.push('separator')
  }
  items.push(
    { label: 'New File…', icon: FilePlus, run: () => void newEntry(t.projectId, dir, 'file') },
    { label: 'New Folder…', icon: FolderPlus, run: () => void newEntry(t.projectId, dir, 'folder') },
  )
  if (t.path) {
    items.push(
      { label: 'Rename…', icon: Pencil, shortcut: 'F2', run: () => void renameEntry(t) },
      { label: 'Duplicate…', icon: Copy, run: () => void duplicateEntry(t) },
      { label: 'Delete…', icon: Trash2, shortcut: 'Delete', danger: true, run: () => void deleteEntry(t) },
    )
  }
  items.push(
    'separator',
    { label: 'Copy Path', icon: Link2, run: () => void copyToClipboard(abs, 'path') },
    { label: 'Copy Relative Path', icon: Link2, disabled: !t.path, run: () => void copyToClipboard(t.path, 'relative path') },
    { label: 'Open in Terminal', icon: SquareTerminal, run: () => void openTerminalAt(t.projectId, dir) },
  )
  if (t.isDir) items.push({ label: 'Find in Folder…', icon: TextSearch, run: () => showSearch(t.projectId, undefined, t.path ? `${t.path}/**` : '') })
  items.push('separator', { label: 'Show History', icon: History, run: () => showHistory(t.projectId, t.path) })
  items.push({ label: 'Show Local History', icon: Clock, run: () => showLocalHistory(t.projectId, t.path, t.isDir || !t.path) })
  if (!t.isDir) items.push({ label: 'Compare with HEAD', icon: GitCompare, run: () => compareWithHead(t.projectId, t.path) })
  items.push('separator', { label: 'Ask Agent About This', icon: Bot, run: () => askAboutFile(t.projectId, t.path || '.') })
  if (!t.isDir) items.push({ label: 'Download', icon: Download, run: () => downloadFile(t.projectId, t.path) })
  if (t.isDir) items.push({ label: 'Reload from Disk', icon: RefreshCw, run: () => refreshDirs(t.projectId, [t.path]) })
  return items
}

export { bufferKey }
