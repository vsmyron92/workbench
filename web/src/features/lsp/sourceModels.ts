// Models of library files (`lsp-src://<pid>/<path>`), reference counted: the source
// panels showing them, and short holds for Monaco's previews of definitions (Ctrl+hover,
// peek), which need the target's model and fail without one. Project files are held the
// same way through `modelAccess.ensureModel` (the files slice's buffers).

import type { editor } from 'monaco-editor'
import { ensureModel, parseModelUri } from '@/features/files/modelAccess'
import { displayPath } from './convert'
import { textOf } from './nav'

type MonacoNs = (typeof import('@/lib/monacoSetup'))['monaco']

const sources = new Map<string, { model: editor.ITextModel; refs: number }>()

export function acquireSource(monaco: MonacoNs, uri: string, content: string, languageFor: (p: string) => string): editor.ITextModel {
  const e = sources.get(uri)
  if (e && !e.model.isDisposed()) {
    e.refs++
    return e.model
  }
  const u = monaco.Uri.parse(uri)
  const model = monaco.editor.getModel(u) ?? monaco.editor.createModel(content, languageFor(displayPath(uri)), u)
  sources.set(uri, { model, refs: 1 })
  return model
}

export function releaseSource(uri: string) {
  const e = sources.get(uri)
  if (!e) return
  if (--e.refs > 0) return
  sources.delete(uri)
  // After the editor let go of it.
  setTimeout(() => !e.model.isDisposed() && e.model.dispose(), 0)
}

/** Preview holds: released a minute after the last use. */
const holds = new Map<string, { release: () => void; timer: ReturnType<typeof setTimeout> }>()
const HOLD_MS = 60_000

/** Hold `uri` for a minute more; a second reference to something held is given back at once. */
function hold(uri: string, release: () => void) {
  const prev = holds.get(uri)
  if (prev) {
    clearTimeout(prev.timer)
    release()
  }
  const h = prev ?? { release, timer: undefined as unknown as ReturnType<typeof setTimeout> }
  h.timer = setTimeout(() => {
    holds.delete(uri)
    h.release()
  }, HOLD_MS)
  holds.set(uri, h)
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | null> {
  return Promise.race([p, new Promise<null>((r) => setTimeout(() => r(null), ms))])
}

/**
 * Make sure the targets of a definition-like answer have models, so Monaco can show
 * their preview (at most 10 files; a slow read is not waited for).
 */
export async function ensurePreviewModels(monaco: MonacoNs, uris: string[]): Promise<void> {
  const { languageFor } = await import('@/lib/monacoSetup')
  const distinct = [...new Set(uris)].slice(0, 10)
  await Promise.all(
    distinct.map(async (uri) => {
      const u = monaco.Uri.parse(uri)
      const existing = holds.get(uri)
      if (monaco.editor.getModel(u)) {
        // Someone shows it; keep our hold (if any) fresh.
        if (existing) hold(uri, () => {})
        return
      }
      const ref = parseModelUri(uri)
      if (ref?.projectId) {
        const got = await withTimeout(
          ensureModel(ref.projectId, ref.path).catch(() => null),
          1500,
        )
        if (got) hold(uri, got.release)
      } else if (uri.startsWith('lsp-src://')) {
        const text = await withTimeout(textOf(uri), 1500)
        if (text !== null && !monaco.editor.getModel(u)) {
          acquireSource(monaco, uri, text, languageFor)
          hold(uri, () => releaseSource(uri))
        }
      }
    }),
  )
}
