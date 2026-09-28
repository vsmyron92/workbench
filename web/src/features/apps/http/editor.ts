// HTTP Client in the editor: above every request of an .http file, "▶ Send
// Request" and its environment (click to change it); Ctrl+Enter sends the request
// at the caret. An unsaved file is saved first: the server runs what is on disk.

import type { languages } from 'monaco-editor'
import { isDirty, modelUriString, parseModelUri, saveModel } from '@/features/files/modelAccess'
import { openPanel, toast, toastError } from '@/shell/actions'
import { showMenu } from '@/ui'
import { httpApi } from './api'
import { scanRequests } from './scan'
import { addRun, selectedEnv, selectEnv, useHttp } from './store'

let installing: Promise<void> | null = null
/** Where the pointer last went down (a CodeLens click carries no position). */
let lastPointer = { x: window.innerWidth / 2, y: 160 }

export function installHttpEditor(): Promise<void> {
  installing ??= install()
  return installing
}

async function install() {
  const { monaco } = await import('@/lib/monacoSetup')
  window.addEventListener('pointerdown', (e) => (lastPointer = { x: e.clientX, y: e.clientY }), true)
  const envChanged = new monaco.Emitter<languages.CodeLensProvider>()
  monaco.editor.registerCommand('wb.http.send', (_accessor, uri: string, line: number) => void sendAt(uri, line))
  monaco.editor.registerCommand('wb.http.env', (_accessor, uri: string) => void chooseEnv(uri))
  const lensProvider: languages.CodeLensProvider = {
    onDidChange: envChanged.event,
    provideCodeLenses: (model) => {
      const ref = parseModelUri(model.uri)
      if (!ref?.projectId) return { lenses: [], dispose: () => {} }
      const env = selectedEnv(ref.projectId)
      const uri = model.uri.toString()
      const lenses = scanRequests(model.getValue()).flatMap((r) => {
        const range = new monaco.Range(r.line, 1, r.line, 1)
        return [
          { range, command: { id: 'wb.http.send', title: '▶ Send Request', arguments: [uri, r.line] } },
          { range, command: { id: 'wb.http.env', title: env ? `Environment: ${env}` : 'No environment', arguments: [uri] } },
        ]
      })
      return { lenses, dispose: () => {} }
    },
  }
  monaco.languages.registerCodeLensProvider('http', lensProvider)
  // Another environment: the lenses say so.
  useHttp.subscribe((s, p) => {
    if (s.envTick !== p.envTick) envChanged.fire(lensProvider)
  })
  monaco.editor.addEditorAction({
    id: 'wb.http.sendAtCaret',
    label: 'Send Request',
    keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter],
    precondition: "editorLangId == 'http'",
    run: (ed) => {
      const m = ed.getModel()
      const pos = ed.getPosition()
      if (m && pos) void sendAt(m.uri.toString(), pos.lineNumber)
    },
  })
}

/** The environments of a file; with exactly one and none chosen, that one is chosen. */
async function envsOf(projectId: string, path: string): Promise<string[]> {
  const r = await httpApi.envs(projectId, path)
  if (r.envs.length === 1 && !selectedEnv(projectId)) selectEnv(projectId, r.envs[0])
  return r.envs
}

async function chooseEnv(uri: string) {
  const ref = parseModelUri(uri)
  if (!ref?.projectId) return
  const pid = ref.projectId
  try {
    const envs = await envsOf(pid, ref.path)
    const cur = selectedEnv(pid)
    showMenu({ clientX: lastPointer.x, clientY: lastPointer.y }, [
      ...envs.map((e) => ({ label: `${e === cur ? '✓ ' : ''}${e}`, run: () => selectEnv(pid, e) })),
      ...(envs.length ? (['separator'] as const) : []),
      { label: `${cur ? '' : '✓ '}No environment`, run: () => selectEnv(pid, null) },
      ...(envs.length ? [] : [{ label: 'Add http-client.env.json next to the file to define environments', disabled: true, run: () => {} }]),
    ])
  } catch (e) {
    toastError(e, 'Could not read the environments')
  }
}

let nextId = 1

async function sendAt(uri: string, line: number) {
  const ref = parseModelUri(uri)
  if (!ref?.projectId) return toast('info', 'The HTTP Client works on project files')
  const pid = ref.projectId
  if (isDirty(pid, ref.path) && !(await saveModel(pid, ref.path))) return toast('warning', 'Save the file first: requests run from the file on disk')
  const id = nextId++
  try {
    await envsOf(pid, ref.path)
  } catch {
    /* sending reports the problem */
  }
  const env = selectedEnv(pid)
  const { monaco } = await import('@/lib/monacoSetup')
  const text = monaco.editor.getModel(monaco.Uri.parse(uri))?.getValue() ?? ''
  const mark = scanRequests(text)
    .filter((r) => r.line <= line)
    .pop()
  addRun({ id, projectId: pid, pending: { path: ref.path, line, method: mark?.method ?? 'GET', env, at: Date.now() } })
  openPanel({ kind: 'httpResponse', id: `http:${pid}`, title: 'HTTP Response', params: { projectId: pid }, position: 'right' })
  try {
    const result = await httpApi.run(pid, ref.path, line, env)
    addRun({ id, projectId: pid, result })
  } catch (e) {
    addRun({ id, projectId: pid, error: e instanceof Error ? e.message : String(e), pending: { path: ref.path, line, method: mark?.method ?? 'GET', env, at: Date.now() } })
  }
}

/** Send a request again (the response panel's Run Again). */
export function sendAgain(projectId: string, path: string, line: number) {
  return sendAt(modelUriString(projectId, path), line)
}
