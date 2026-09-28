// Shift+F6 Rename: the new name, then a preview of every edit across files (per-file
// checkboxes) before anything changes. Applied edits go into the files' buffers
// (unsaved; Save All in the toast).

import { useEffect, useMemo, useRef, useState } from 'react'
import { FileCode, Library } from 'lucide-react'
import { Button, Checkbox, ErrorBox, Input, Modal, Spinner } from '@/ui'
import type { LspTextEdit } from './api'
import { lsp } from './client'
import { displayPath } from './convert'
import { previewParts } from './logic'
import { lineOf, textOf } from './nav'
import { usePopups, type RenameState } from './store'
import { applyWorkspaceEdit, editsByUri, reportApplied } from './workspaceEdit'

export function RenameHost() {
  const r = usePopups((s) => s.rename)
  if (!r) return null
  return r.edit ? <Preview state={r} /> : <AskName state={r} />
}

function close(state: RenameState) {
  usePopups.getState().set({ rename: null })
  state.editor?.focus()
}

function valid(name: string) {
  return name.trim().length > 0 && !/[\s\n]/.test(name.trim())
}

function AskName({ state }: { state: RenameState }) {
  const [name, setName] = useState(state.oldName)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => {
    input.current?.select()
  }, [])
  const submit = async () => {
    const newName = name.trim()
    if (!valid(newName) || newName === state.oldName) return
    const conn = lsp.connection(state.projectId)
    if (!conn) return
    setBusy(true)
    setError(null)
    try {
      const r = await conn.request<Parameters<typeof editsByUri>[0] | null>('textDocument/rename', { textDocument: { uri: state.uri }, position: state.position, newName }, { server: state.server })
      if (!r.result || !editsByUri(r.result).files.size) {
        setError(new Error('The language server found nothing to rename'))
        setBusy(false)
        return
      }
      usePopups.getState().set({ rename: { ...state, edit: r.result, newName } })
    } catch (e) {
      setError(e)
      setBusy(false)
    }
  }
  return (
    <Modal
      title={`Rename ${state.oldName}`}
      onClose={() => close(state)}
      footer={
        <>
          <Button onClick={() => close(state)}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!valid(name) || name.trim() === state.oldName} onClick={() => void submit()}>
            Preview
          </Button>
        </>
      }
    >
      <div className="lsp-rename">
        <label className="wb-small wb-muted" htmlFor="lsp-rename-input">
          New name
        </label>
        <Input
          id="lsp-rename-input"
          ref={input}
          autoFocus
          value={name}
          spellCheck={false}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              void submit()
            }
          }}
        />
        <div className="wb-small wb-subtle">Every usage the language server knows is renamed; you see the changes before they are made.</div>
        {!!error && <ErrorBox error={error} />}
      </div>
    </Modal>
  )
}

function Preview({ state }: { state: RenameState }) {
  const { files, unsupported } = useMemo(() => editsByUri(state.edit!), [state.edit])
  const groups = useMemo(() => [...files.entries()].map(([uri, raw]) => ({ uri, raw })), [files])
  const [texts, setTexts] = useState<Map<string, string | null>>(new Map())
  const [off, setOff] = useState<Set<string>>(() => new Set([...files.keys()].filter((u) => !u.startsWith('file:///'))))
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    let cancelled = false
    for (const u of files.keys()) void textOf(u).then((t) => !cancelled && setTexts((m) => new Map(m).set(u, t)))
    return () => {
      cancelled = true
    }
  }, [files])
  const total = [...files.values()].reduce((n, e) => n + e.length, 0)
  const chosen = [...files.keys()].filter((u) => !off.has(u))
  const apply = async () => {
    setBusy(true)
    const r = await applyWorkspaceEdit(state.edit!, new Set(chosen))
    usePopups.getState().set({ rename: null })
    reportApplied(r, `Rename to ${state.newName}`)
    state.editor?.focus()
  }
  return (
    <Modal
      wide
      title={`Rename ${state.oldName} to ${state.newName}`}
      onClose={() => close(state)}
      footer={
        <>
          <span className="wb-grow wb-small wb-muted">
            {total} occurrence{total === 1 ? '' : 's'} in {files.size} file{files.size === 1 ? '' : 's'}
            {chosen.length !== files.size ? `, ${chosen.length} selected` : ''}
          </span>
          <Button onClick={() => close(state)}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!chosen.length || unsupported.length > 0} onClick={() => void apply()}>
            Do Refactor
          </Button>
        </>
      }
    >
      {unsupported.length > 0 && (
        <div className="wb-error" style={{ marginBottom: 8 }}>
          The server also wants to {unsupported.join(', ')}; Workbench does not move or create files for it.
        </div>
      )}
      <div className="lsp-preview">
        {groups.map((g) => {
          const lib = !g.uri.startsWith('file:///')
          const text = texts.get(g.uri)
          return (
            <div key={g.uri} className="lsp-preview-file">
              <div className="lsp-preview-head">
                <Checkbox
                  checked={!off.has(g.uri)}
                  disabled={lib}
                  onChange={(v) =>
                    setOff((s) => {
                      const n = new Set(s)
                      if (v) n.delete(g.uri)
                      else n.add(g.uri)
                      return n
                    })
                  }
                >
                  {lib ? <Library size={13} /> : <FileCode size={13} />}
                  <span className="wb-ellipsis">{displayPath(g.uri)}</span>
                </Checkbox>
                <span className="wb-subtle wb-small">{lib ? 'read-only library file' : `${g.raw.length}`}</span>
              </div>
              {text === undefined && (
                <div className="lsp-preview-line">
                  <Spinner size={10} />
                </div>
              )}
              {text !== undefined &&
                sortedEdits(g.raw).map((e, i) => {
                  const line = lineOf(text ?? null, e.range.start.line)
                  const end = e.range.end.line === e.range.start.line ? e.range.end.character : line.length
                  const p = previewParts(line, e.range.start.character, end, 160)
                  return (
                    <div key={i} className="lsp-preview-line">
                      <span className="lsp-preview-num">{e.range.start.line + 1}</span>
                      <span className="lsp-preview-code wb-ellipsis">
                        {p.before}
                        <del>{p.match}</del>
                        <ins>{e.newText}</ins>
                        {p.after}
                      </span>
                    </div>
                  )
                })}
            </div>
          )
        })}
      </div>
    </Modal>
  )
}

function sortedEdits(edits: LspTextEdit[]): LspTextEdit[] {
  return [...edits].sort((a, b) => a.range.start.line - b.range.start.line || a.range.start.character - b.range.start.character)
}
