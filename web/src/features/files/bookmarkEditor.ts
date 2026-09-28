// Bookmarks in an editor: gutter marks (the glyph margin's right lane, beside
// breakpoints) that follow edits, whose new lines go back to the store.

import type { editor, IDisposable } from 'monaco-editor'
import { MNEMONICS, useBookmarks, type Bookmark } from './bookmarks'

type Monaco = typeof import('monaco-editor')

let styled = false
/** One rule per mnemonic: a decoration class cannot carry text. */
function injectMnemonicStyles() {
  if (styled || typeof document === 'undefined') return
  styled = true
  const style = document.createElement('style')
  style.textContent = MNEMONICS.map((m) => `.monaco-editor .wb-bm-glyph.m-${m}::after { content: '${m}'; }`).join('\n')
  document.head.appendChild(style)
}

export function attachBookmarks(ed: editor.IStandaloneCodeEditor, monaco: Monaco, file: () => { projectId: string | null; path: string }): IDisposable {
  injectMnemonicStyles()
  const marks = ed.createDecorationsCollection()
  let shown: Bookmark[] = []
  const render = () => {
    const { projectId, path } = file()
    const model = ed.getModel()
    if (!model) {
      marks.clear()
      shown = []
      return
    }
    const max = model.getLineCount()
    shown = useBookmarks.getState().forFile(projectId, path).filter((b) => b.line >= 1 && b.line <= max)
    marks.set(
      shown.map((b) => ({
        range: new monaco.Range(b.line, 1, b.line, 1),
        options: {
          glyphMarginClassName: b.mnemonic ? `wb-bm-glyph m-${b.mnemonic}` : 'wb-bm-glyph',
          glyphMargin: { position: monaco.editor.GlyphMarginLane.Right },
          glyphMarginHoverMessage: { value: b.mnemonic ? `Bookmark ${b.mnemonic}` : 'Bookmark' },
          stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
        },
      })),
    )
  }
  render()
  const unsubscribe = useBookmarks.subscribe((s, prev) => {
    if (s.list !== prev.list) render()
  })
  // Edits move the marks; write their lines back once typing pauses.
  let timer: number | undefined
  const content = ed.onDidChangeModelContent(() => {
    window.clearTimeout(timer)
    timer = window.setTimeout(() => {
      const ranges = marks.getRanges()
      useBookmarks.getState().moveLines(shown.map((b, i) => ({ id: b.id, line: ranges[i]?.startLineNumber ?? b.line })))
    }, 400)
  })
  const model = ed.onDidChangeModel(render)
  return {
    dispose: () => {
      window.clearTimeout(timer)
      unsubscribe()
      content.dispose()
      model.dispose()
      marks.clear()
    },
  }
}
