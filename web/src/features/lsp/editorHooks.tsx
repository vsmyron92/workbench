// Every Monaco editor gets the CLion keymap for code intelligence (as editor actions:
// they apply only while that editor has focus), the "Enable code intelligence" banner
// for project files, and cross-file navigation opens the `editor` / `lsp.source`
// panel (`registerEditorOpener`).

import type { editor, IDisposable } from 'monaco-editor'
import { createRoot } from 'react-dom/client'
import { QueryClientProvider } from '@tanstack/react-query'
import { parseModelUri } from '@/features/files/modelAccess'
import { lspQueryClient } from './api'
import { fileStructure, findUsages, gotoDeclaration, gotoImplementation, gotoTypeDeclaration, renameAt, reformat, showHierarchy, showUsages } from './actions'
import { useLspRuntime, type LspClient } from './client'
import { toLspPosition, toLspRange } from './convert'
import { EnableBanner } from './EnableBanner'
import { openLocation } from './nav'

type MonacoNs = (typeof import('@/lib/monacoSetup'))['monaco']

export function installEditorHooks(monaco: MonacoNs, client: LspClient) {
  monaco.editor.registerEditorOpener({
    openCodeEditor(_source, resource, selectionOrPosition) {
      if (resource.scheme !== 'file' && resource.scheme !== 'lsp-src') return false
      let range
      if (selectionOrPosition) {
        range =
          'startLineNumber' in selectionOrPosition
            ? toLspRange(selectionOrPosition)
            : { start: toLspPosition(selectionOrPosition), end: toLspPosition(selectionOrPosition) }
      }
      openLocation(resource.toString(), range)
      return true
    },
  })
  // The event fires inside Monaco's base constructor, before the standalone editor
  // has its keybinding service: actions added then would lose their keys.
  monaco.editor.onDidCreateEditor((ed) => queueMicrotask(() => hookEditor(monaco, client, ed)))
  for (const ed of monaco.editor.getEditors()) hookEditor(monaco, client, ed)
}

const hooked = new WeakSet<editor.ICodeEditor>()

function hookEditor(monaco: MonacoNs, client: LspClient, codeEditor: editor.ICodeEditor) {
  // Editors made by `monaco.editor.create` (and diff editors' sides) are standalone ones.
  const ed = codeEditor as editor.IStandaloneCodeEditor
  if (hooked.has(ed) || typeof ed.addAction !== 'function') return
  hooked.add(ed)
  client.editors.add(ed)
  const disposables: IDisposable[] = []
  const { KeyMod, KeyCode } = monaco
  const isCode = ed.createContextKey<boolean>('wbLspFile', false)
  const update = () => {
    const uri = ed.getModel()?.uri
    isCode.set(!!uri && (uri.scheme === 'lsp-src' || (uri.scheme === 'file' && !!parseModelUri(uri)?.projectId)))
  }
  update()
  const noteActive = () => {
    client.lastEditor = ed
    useLspRuntime.setState({ activeUri: ed.getModel()?.uri.toString() ?? null })
  }
  disposables.push(ed.onDidChangeModel(update))
  disposables.push(ed.onDidFocusEditorText(noteActive))
  disposables.push(ed.onDidChangeModel(() => ed.hasTextFocus() && noteActive()))
  // An editor focused before code intelligence hooked it (it installs once Monaco is
  // loaded) sent its focus event already.
  if (ed.hasTextFocus()) noteActive()
  // Several declarations on Ctrl+click: go to the first (Ctrl+B offers a chooser itself).
  // References keep Monaco's peek (Shift+F12).
  ed.updateOptions({
    gotoLocation: { multipleDefinitions: 'goto', multipleTypeDefinitions: 'goto', multipleDeclarations: 'goto', multipleImplementations: 'goto' },
    // Colour from the language servers' semantic tokens where they give them.
    'semanticHighlighting.enabled': true,
  })

  const add = (a: { id: string; label: string; keys: number[]; run: (e: editor.ICodeEditor) => unknown; menu?: number; always?: boolean }) =>
    disposables.push(
      ed.addAction({
        id: a.id,
        label: a.label,
        keybindings: a.keys,
        precondition: a.always ? undefined : 'wbLspFile',
        contextMenuGroupId: a.menu !== undefined ? '1_lsp' : undefined,
        contextMenuOrder: a.menu,
        run: (e: editor.ICodeEditor) => void a.run(e),
      }),
    )
  add({ id: 'wb.lsp.gotoDeclaration', label: 'Go to Declaration or Usages', keys: [KeyMod.CtrlCmd | KeyCode.KeyB], run: gotoDeclaration, menu: 1 })
  add({ id: 'wb.lsp.gotoImplementation', label: 'Go to Implementation(s)', keys: [KeyMod.CtrlCmd | KeyMod.Alt | KeyCode.KeyB], run: gotoImplementation, menu: 2 })
  add({ id: 'wb.lsp.gotoTypeDeclaration', label: 'Go to Type Declaration', keys: [KeyMod.CtrlCmd | KeyMod.Shift | KeyCode.KeyB], run: gotoTypeDeclaration, menu: 3 })
  add({ id: 'wb.lsp.findUsages', label: 'Find Usages', keys: [KeyMod.Alt | KeyCode.F7], run: findUsages, menu: 4 })
  add({ id: 'wb.lsp.showUsages', label: 'Show Usages', keys: [KeyMod.CtrlCmd | KeyMod.Alt | KeyCode.F7], run: showUsages, menu: 5 })
  add({ id: 'wb.lsp.rename', label: 'Rename…', keys: [KeyMod.Shift | KeyCode.F6], run: renameAt, menu: 6 })
  add({ id: 'wb.lsp.fileStructure', label: 'File Structure', keys: [KeyMod.CtrlCmd | KeyCode.F12], run: fileStructure, menu: 7 })
  add({ id: 'wb.lsp.reformat', label: 'Reformat Code', keys: [KeyMod.CtrlCmd | KeyMod.Alt | KeyCode.KeyL], run: reformat, menu: 8, always: true })
  add({ id: 'wb.lsp.callHierarchy', label: 'Call Hierarchy', keys: [KeyMod.CtrlCmd | KeyMod.Alt | KeyCode.KeyH], run: (e) => showHierarchy(e, 'call') })
  // CLion's Ctrl+H (Monaco's Replace moves to Ctrl+R, as in CLion: lib/monacoSetup.ts).
  add({ id: 'wb.lsp.typeHierarchy', label: 'Type Hierarchy', keys: [KeyMod.CtrlCmd | KeyCode.KeyH], run: (e) => showHierarchy(e, 'type') })
  add({
    id: 'wb.lsp.contextActions',
    label: 'Show Context Actions',
    keys: [KeyMod.Alt | KeyCode.Enter],
    // A command, not an action, in Monaco 0.57: triggered by id.
    run: (e) => e.trigger('keyboard', 'editor.action.quickFix', null),
    always: true,
  })
  add({ id: 'wb.lsp.nextError', label: 'Next Highlighted Error', keys: [KeyCode.F2], run: (e) => e.trigger('keyboard', 'editor.action.marker.next', null), always: true })
  add({ id: 'wb.lsp.prevError', label: 'Previous Highlighted Error', keys: [KeyMod.Shift | KeyCode.F2], run: (e) => e.trigger('keyboard', 'editor.action.marker.prev', null), always: true })

  const banner = attachBanner(ed)
  ed.onDidDispose(() => {
    disposables.splice(0).forEach((d) => d.dispose())
    banner.dispose()
    client.editors.delete(ed)
    if (client.lastEditor === ed) client.lastEditor = null
  })
}

/** The banner lives in an overlay at the top, above a view zone of its height. */
function attachBanner(ed: editor.ICodeEditor): IDisposable {
  const dom = document.createElement('div')
  dom.className = 'lsp-banner-host'
  const widget: editor.IOverlayWidget = {
    getId: () => 'wb.lsp.banner',
    getDomNode: () => dom,
    getPosition: () => ({ preference: { top: 0, left: 0 } }),
  }
  ed.addOverlayWidget(widget)
  const root = createRoot(dom)
  let zone: string | null = null
  let height = 0
  const setHeight = (h: number) => {
    if (h === height) return
    height = h
    ed.changeViewZones((acc) => {
      if (zone) acc.removeZone(zone)
      zone = h ? acc.addZone({ afterLineNumber: 0, heightInPx: h, domNode: document.createElement('div'), suppressMouseDown: true }) : null
    })
  }
  const layout = () => {
    const l = ed.getLayoutInfo()
    dom.style.width = `${Math.max(0, l.width - l.verticalScrollbarWidth)}px`
  }
  const render = () => {
    const model = ed.getModel()
    const ref = model ? parseModelUri(model.uri) : null
    const qc = lspQueryClient()
    if (!ref?.projectId || !qc || !model) {
      root.render(null)
      setHeight(0)
      return
    }
    root.render(
      <QueryClientProvider client={qc}>
        <EnableBanner projectId={ref.projectId} path={ref.path} language={model.getLanguageId()} onHeight={setHeight} />
      </QueryClientProvider>,
    )
  }
  layout()
  render()
  const subs = [ed.onDidChangeModel(render), ed.onDidLayoutChange(layout)]
  return {
    dispose: () => {
      subs.forEach((s) => s.dispose())
      // Unmounting during React's own render phase is not allowed: after it.
      setTimeout(() => root.unmount(), 0)
    },
  }
}
