// CLion's editing keys in every Monaco editor, or Monaco's own (VS Code) ones:
// Settings › General › Editor keymap. Keys CLion gives to navigation and code
// intelligence (Ctrl+B, Ctrl+H, Alt+F7, F11…) are the features' own editor actions
// and stay either way; Ctrl+R (Replace) is set in monacoSetup.ts.
//
// Not mappable in a browser: Ctrl+W / Ctrl+Shift+W (Extend / Shrink Selection: the
// browser closes the tab; Monaco's Shift+Alt+Right / Left do it), Ctrl+N / Ctrl+Shift+N,
// Ctrl+Tab and Ctrl+F4.

import type { IDisposable } from 'monaco-editor'
import { useUi } from '@/state/store'

type Monaco = typeof import('monaco-editor/editor')

export type EditorKeymap = 'clion' | 'vscode'

/** Only while typing in the editor (not in its find widget, rename box or suggestions). */
const EDITING = 'editorTextFocus && !editorReadonly'

function clionRules(monaco: Monaco) {
  const { KeyMod: M, KeyCode: K } = monaco
  return [
    { keybinding: M.CtrlCmd | K.KeyD, command: 'editor.action.copyLinesDownAction', when: EDITING },
    { keybinding: M.CtrlCmd | K.KeyY, command: 'editor.action.deleteLines', when: EDITING },
    { keybinding: M.CtrlCmd | M.Shift | K.KeyZ, command: 'redo', when: 'editorTextFocus' },
    { keybinding: M.Alt | K.KeyJ, command: 'editor.action.addSelectionToNextFindMatch', when: 'editorFocus' },
    { keybinding: M.Alt | M.Shift | K.KeyJ, command: 'cursorUndo', when: 'editorTextFocus' },
    { keybinding: M.CtrlCmd | M.Alt | M.Shift | K.KeyJ, command: 'editor.action.selectHighlights', when: 'editorFocus' },
    { keybinding: M.CtrlCmd | M.Shift | K.KeyJ, command: 'editor.action.joinLines', when: EDITING },
    { keybinding: M.CtrlCmd | M.Shift | K.UpArrow, command: 'editor.action.moveLinesUpAction', when: EDITING },
    { keybinding: M.CtrlCmd | M.Shift | K.DownArrow, command: 'editor.action.moveLinesDownAction', when: EDITING },
    { keybinding: M.Alt | M.Shift | K.UpArrow, command: 'editor.action.moveLinesUpAction', when: EDITING },
    { keybinding: M.Alt | M.Shift | K.DownArrow, command: 'editor.action.moveLinesDownAction', when: EDITING },
    { keybinding: M.Shift | K.Enter, command: 'editor.action.insertLineAfter', when: `${EDITING} && !suggestWidgetVisible` },
    { keybinding: M.CtrlCmd | M.Alt | K.Enter, command: 'editor.action.insertLineBefore', when: EDITING },
    { keybinding: M.CtrlCmd | K.KeyQ, command: 'editor.action.showHover', when: 'editorTextFocus' },
    { keybinding: M.CtrlCmd | M.Shift | K.Slash, command: 'editor.action.blockComment', when: EDITING },
    { keybinding: M.CtrlCmd | M.Alt | K.KeyO, command: 'editor.action.organizeImports', when: EDITING },
    { keybinding: M.CtrlCmd | M.Alt | K.KeyI, command: 'editor.action.reindentselectedlines', when: EDITING },
    { keybinding: M.CtrlCmd | M.Shift | K.KeyM, command: 'editor.action.jumpToBracket', when: 'editorTextFocus' },
    { keybinding: M.CtrlCmd | K.NumpadAdd, command: 'editor.unfold', when: 'editorTextFocus' },
    { keybinding: M.CtrlCmd | K.NumpadSubtract, command: 'editor.fold', when: 'editorTextFocus' },
    { keybinding: M.CtrlCmd | M.Shift | K.NumpadAdd, command: 'editor.unfoldAll', when: 'editorTextFocus' },
    { keybinding: M.CtrlCmd | M.Shift | K.NumpadSubtract, command: 'editor.foldAll', when: 'editorTextFocus' },
  ]
}

/** CLion's Toggle Case: upper case, unless the selection already is. */
export function toggledCase(text: string): string {
  return text === text.toUpperCase() ? text.toLowerCase() : text.toUpperCase()
}

function clionActions(monaco: Monaco): IDisposable[] {
  const { KeyMod: M, KeyCode: K } = monaco
  return [
    monaco.editor.addEditorAction({
      id: 'wb.toggleCase',
      label: 'Toggle Case',
      keybindings: [M.CtrlCmd | M.Shift | K.KeyU],
      precondition: EDITING,
      run: (ed) => {
        const model = ed.getModel()
        const sels = ed.getSelections()
        if (!model || !sels) return
        const edits = sels.map((s) => {
          // Nothing selected: the word at the caret, as in CLion.
          const w = s.isEmpty() ? model.getWordAtPosition(s.getStartPosition()) : null
          const range = w ? new monaco.Range(s.startLineNumber, w.startColumn, s.startLineNumber, w.endColumn) : s
          return { range, text: toggledCase(model.getValueInRange(range)) }
        })
        ed.pushUndoStop()
        ed.executeEdits('wb.toggleCase', edits, sels)
        ed.pushUndoStop()
      },
    }),
  ]
}

/** Apply the preference now and whenever it changes. */
export function installEditorKeymap(monaco: Monaco) {
  let active: IDisposable[] = []
  let current: EditorKeymap | null = null
  const apply = (keymap: EditorKeymap) => {
    if (keymap === current) return
    current = keymap
    active.splice(0).forEach((d) => d.dispose())
    if (keymap === 'clion') active = [monaco.editor.addKeybindingRules(clionRules(monaco)), ...clionActions(monaco)]
  }
  apply(useUi.getState().prefs.editorKeymap ?? 'clion')
  useUi.subscribe((s) => apply(s.prefs.editorKeymap ?? 'clion'))
}
