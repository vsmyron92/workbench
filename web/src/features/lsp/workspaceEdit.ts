// Edits a language server wants (rename, code actions, `workspace/applyEdit`) go into
// the editor buffers of every affected file: open ones directly, others through
// `modelAccess.ensureModel` (read from disk into a buffer). They become dirty like any
// edit and are saved by the user (Save All), never behind their back. A buffer opened
// only for an edit is let go once it is saved or reverted.

import type { editor } from 'monaco-editor'
import { toast, toastError } from '@/shell/actions'
import { ensureModel, isDirty, isReadOnly, onBuffersChange, parseModelUri, peekModel, saveModel, type FileRef } from '@/features/files/modelAccess'
import type { LspCodeAction, LspCommand, LspTextEdit, LspWorkspaceEdit } from './api'
import { lsp } from './client'
import { displayPath, sortEditsDescending, toRange } from './convert'

/** Buffers opened only to receive edits: released once clean. */
const held = new Map<string, { ref: FileRef; release: () => void }>()
let watching = false

function watchHeld() {
  if (watching) return
  watching = true
  onBuffersChange(() => {
    for (const [k, h] of held) {
      if (!isDirty(h.ref.projectId, h.ref.path)) {
        held.delete(k)
        // After this change settles (the save that made it clean).
        setTimeout(() => h.release(), 0)
      }
    }
  })
}

/** Text edits per file, from `changes` or `documentChanges`. Resource operations are refused. */
export function editsByUri(edit: LspWorkspaceEdit): { files: Map<string, LspTextEdit[]>; unsupported: string[] } {
  const files = new Map<string, LspTextEdit[]>()
  const unsupported: string[] = []
  const add = (uri: string, edits: LspTextEdit[]) => files.set(uri, [...(files.get(uri) ?? []), ...edits])
  if (edit.documentChanges) {
    for (const c of edit.documentChanges) {
      if ('textDocument' in c) add(c.textDocument.uri, c.edits)
      else unsupported.push(`${c.kind} ${displayPath(c.uri ?? c.newUri ?? c.oldUri ?? '')}`)
    }
  } else {
    for (const [uri, edits] of Object.entries(edit.changes ?? {})) add(uri, edits)
  }
  return { files, unsupported }
}

export interface ApplyResult {
  applied: boolean
  failureReason?: string
  /** Files changed. */
  files: FileRef[]
  /** Files that were not open and now have a (dirty) buffer. */
  opened: FileRef[]
}

/** Apply edits to buffers. All files are prepared first; nothing changes if one cannot be. */
export async function applyWorkspaceEdit(edit: LspWorkspaceEdit, only?: Set<string>): Promise<ApplyResult> {
  const { files, unsupported } = editsByUri(edit)
  if (unsupported.length) {
    return { applied: false, failureReason: `Workbench does not create, rename or delete files from a language server (${unsupported.join(', ')})`, files: [], opened: [] }
  }
  const targets: { uri: string; ref: FileRef; edits: LspTextEdit[]; model: editor.ITextModel; release?: () => void }[] = []
  const fail = (reason: string): ApplyResult => {
    targets.forEach((t) => t.release?.())
    return { applied: false, failureReason: reason, files: [], opened: [] }
  }
  for (const [uri, edits] of files) {
    if (only && !only.has(uri)) continue
    if (!edits.length) continue
    const ref = parseModelUri(uri)
    if (!ref || !ref.projectId) return fail(`${displayPath(uri)} is outside the project and read-only`)
    if (isReadOnly(ref.projectId, ref.path) && peekModel(ref.projectId, ref.path)) return fail(`${ref.path} is read-only`)
    const open = peekModel(ref.projectId, ref.path)
    if (open) {
      targets.push({ uri, ref, edits, model: open })
      continue
    }
    try {
      const { model, release } = await ensureModel(ref.projectId, ref.path)
      targets.push({ uri, ref, edits, model, release })
    } catch (e) {
      return fail(`cannot open ${ref.path}: ${e instanceof Error ? e.message : String(e)}`)
    }
  }
  for (const t of targets) {
    const ops = sortEditsDescending(t.edits).map((e) => ({ range: toRange(e.range), text: e.newText, forceMoveMarkers: true }))
    t.model.pushStackElement()
    t.model.pushEditOperations([], ops, () => null)
    t.model.pushStackElement()
  }
  const opened: FileRef[] = []
  for (const t of targets) {
    if (!t.release) continue
    const key = `${t.ref.projectId}:${t.ref.path}`
    if (held.has(key) || !isDirty(t.ref.projectId, t.ref.path)) t.release()
    else {
      held.set(key, { ref: t.ref, release: t.release })
      opened.push(t.ref)
    }
  }
  watchHeld()
  return { applied: true, files: targets.map((t) => t.ref), opened }
}

/** Save every file an edit changed (the toast's Save All). */
export async function saveAll(files: FileRef[]): Promise<void> {
  let failed = 0
  for (const f of files) if (isDirty(f.projectId, f.path) && !(await saveModel(f.projectId, f.path))) failed++
  if (failed) toast('warning', `${failed} file${failed > 1 ? 's were' : ' was'} not saved`)
  else toast('success', `Saved ${files.length} file${files.length > 1 ? 's' : ''}`, { timeout: 2500 })
}

/** Tell the user what an edit did, with Save All. */
export function reportApplied(r: ApplyResult, what: string) {
  if (!r.applied) {
    toast('warning', `${what} was not applied`, { detail: r.failureReason })
    return
  }
  if (r.files.length <= 1 && !r.opened.length) return
  const notOpen = r.opened.length ? `, ${r.opened.length} not open in an editor` : ''
  toast('success', `${what}: changed ${r.files.length} file${r.files.length > 1 ? 's' : ''}${notOpen} (unsaved)`, {
    timeout: 12_000,
    action: { label: 'Save All', run: () => void saveAll(r.files) },
  })
}

/** A code action from the lightbulb / Alt+Enter: resolve, apply its edit, run its command. */
export async function runCodeAction(pid: string, server: string, action: LspCodeAction) {
  const conn = lsp.connection(pid)
  if (!conn) return
  let a = action
  try {
    const caps = conn.servers.get(server)?.codeActionProvider as { resolveProvider?: boolean } | undefined
    if (!a.edit && caps?.resolveProvider) {
      const r = await conn.request<LspCodeAction>('codeAction/resolve', a, { server })
      if (r.result) a = r.result
    }
    if (a.edit) reportApplied(await applyWorkspaceEdit(a.edit), a.title)
    if (a.command) await executeCommand(pid, server, a.command)
  } catch (e) {
    toastError(e, a.title)
  }
}

/** `workspace/executeCommand` (edits it makes come back as `workspace/applyEdit`). */
export async function executeCommand(pid: string, server: string, command: LspCommand) {
  const conn = lsp.connection(pid)
  if (!conn) return
  const caps = conn.servers.get(server)?.executeCommandProvider as { commands?: string[] } | undefined
  if (!caps?.commands?.includes(command.command)) {
    // Client-side commands of VS Code extensions (rust-analyzer.runSingle, …) mean nothing here.
    toast('info', `${command.title || command.command} is not available in Workbench`)
    return
  }
  try {
    await conn.request('workspace/executeCommand', { command: command.command, arguments: command.arguments }, { server })
  } catch (e) {
    toastError(e, command.title || command.command)
  }
}
