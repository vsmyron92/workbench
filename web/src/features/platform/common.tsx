// Building blocks shared by the settings sections.

import { useEffect, useRef, useState, type ComponentType, type ReactNode } from 'react'
import type { OnMount } from '@monaco-editor/react'
import { CheckCircle2, Copy, Info, Plus, AlertTriangle, X, XCircle } from 'lucide-react'
import { api } from '@/api/client'
import { toast } from '@/shell/actions'
import { useUi } from '@/state/store'
import { IconButton, Input, MonacoEditor, Spinner } from '@/ui'
import type { Diagnostic } from './types'

export function Page({
  title,
  description,
  actions,
  wide,
  children,
}: {
  title: string
  description?: ReactNode
  actions?: ReactNode
  wide?: boolean
  children: ReactNode
}) {
  return (
    <div className={wide ? 'wb-set-page wide' : 'wb-set-page'}>
      <div className="wb-set-page-head">
        <div className="wb-set-page-text">
          <h2 className="wb-set-title">{title}</h2>
          {description && <div className="wb-set-desc">{description}</div>}
        </div>
        {actions && <div className="wb-row wb-set-page-actions">{actions}</div>}
      </div>
      {children}
    </div>
  )
}

export function Group({
  title,
  description,
  actions,
  children,
  flush,
}: {
  title: ReactNode
  description?: ReactNode
  actions?: ReactNode
  /** Content without the bordered box (tables bring their own). */
  flush?: boolean
  children: ReactNode
}) {
  return (
    <section className="wb-set-group">
      <div className="wb-set-group-head">
        <span className="title">{title}</span>
        <span style={{ flex: 1 }} />
        {actions}
      </div>
      {description && <div className="wb-set-group-desc">{description}</div>}
      {flush ? children : <div className="wb-set-box">{children}</div>}
    </section>
  )
}

export function Row({ label, hint, top, children }: { label: ReactNode; hint?: ReactNode; top?: boolean; children: ReactNode }) {
  return (
    <div className={top ? 'wb-set-row top' : 'wb-set-row'}>
      <div>
        <div className="label">{label}</div>
        {hint && <div className="hint">{hint}</div>}
      </div>
      <div className="control">{children}</div>
    </div>
  )
}

export function Note({ tone, icon: I, children }: { tone?: 'warning'; icon?: ComponentType<{ size?: number }>; children: ReactNode }) {
  const Icon = I ?? (tone === 'warning' ? AlertTriangle : Info)
  return (
    <div className={tone ? `wb-set-note ${tone}` : 'wb-set-note'}>
      <Icon size={14} />
      <div className="wb-grow">{children}</div>
    </div>
  )
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T
  options: { value: T; label: ReactNode; icon?: ComponentType<{ size?: number }> }[]
  onChange: (v: T) => void
}) {
  return (
    <div className="wb-seg-wrap">
      <div className="wb-seg" role="radiogroup">
        {options.map((o) => (
          <button key={o.value} role="radio" aria-checked={o.value === value} className={o.value === value ? 'active' : ''} onClick={() => onChange(o.value)}>
            {o.icon && <o.icon size={13} />}
            <span className="wb-seg-label">{o.label}</span>
          </button>
        ))}
      </div>
    </div>
  )
}

export async function copyText(text: string, what = 'Copied') {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', what)
  } catch {
    // Clipboard needs a secure context (https or localhost).
    toast('warning', 'Copy is not available here; select the text instead')
  }
}

export function CodeLine({ text, copy = true }: { text: string; copy?: boolean }) {
  return (
    <div className="wb-set-code">
      <span>{text}</span>
      {copy && <IconButton icon={Copy} size="small" label="Copy" onClick={() => void copyText(text)} />}
    </div>
  )
}

/** Editable list of strings (paths, hosts). `validate` returns an error message or null. */
export function StringList({
  value,
  onChange,
  placeholder,
  validate,
  addLabel = 'Add',
}: {
  value: string[]
  onChange: (v: string[]) => void
  placeholder?: string
  validate?: (v: string) => string | null
  addLabel?: string
}) {
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const add = () => {
    const v = draft.trim()
    if (!v) return
    const err = validate?.(v) ?? null
    if (err) return setError(err)
    if (value.includes(v)) return setError('Already in the list')
    onChange([...value, v])
    setDraft('')
    setError(null)
  }
  return (
    <div className="wb-strlist">
      {value.length === 0 && <div className="wb-strlist-empty">None</div>}
      {value.map((item, i) => (
        <div key={`${item}-${i}`} className="wb-strlist-item">
          <Input small value={item} readOnly />
          <IconButton icon={X} size="small" label={`Remove ${item}`} onClick={() => onChange(value.filter((_, j) => j !== i))} />
        </div>
      ))}
      <div className="wb-strlist-item">
        <Input
          small
          value={draft}
          placeholder={placeholder}
          onChange={(e) => {
            setDraft(e.target.value)
            setError(null)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              add()
            }
          }}
        />
        <IconButton icon={Plus} size="small" label={addLabel} onClick={add} disabled={!draft.trim()} />
      </div>
      {error && <div className="wb-field-error">{error}</div>}
    </div>
  )
}

/** Debounced validation of TOML text on the server (`/api/settings/validate`). */
export function useTomlDiagnostics(text: string | null, kind: 'global' | 'project'): { diag: Diagnostic | null; checking: boolean } {
  const [diag, setDiag] = useState<Diagnostic | null>(null)
  const [checking, setChecking] = useState(false)
  const seq = useRef(0)
  useEffect(() => {
    if (text === null) return
    const my = ++seq.current
    setChecking(true)
    const t = window.setTimeout(() => {
      api
        .post<Diagnostic>('/api/settings/validate', { kind, text })
        .then((d) => my === seq.current && setDiag(d))
        .catch(() => my === seq.current && setDiag(null))
        .finally(() => my === seq.current && setChecking(false))
    }, 350)
    return () => window.clearTimeout(t)
  }, [text, kind])
  return { diag, checking }
}

type MonacoNs = Parameters<OnMount>[1]
type EditorInst = Parameters<OnMount>[0]

/** Monaco TOML editor with server-side validation markers. */
export function TomlEditor({
  value,
  onChange,
  kind,
  readOnly,
  fill,
  height,
  onSave,
  diagnostic,
}: {
  value: string
  onChange?: (v: string) => void
  kind: 'global' | 'project'
  readOnly?: boolean
  fill?: boolean
  height?: number
  /** Ctrl/Cmd+S inside the editor. */
  onSave?: () => void
  diagnostic?: Diagnostic | null
}) {
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const refs = useRef<{ editor: EditorInst; monaco: MonacoNs } | null>(null)
  const saveRef = useRef(onSave)
  saveRef.current = onSave

  useEffect(() => {
    const r = refs.current
    const model = r?.editor.getModel()
    if (!r || !model) return
    const markers =
      diagnostic && !diagnostic.ok && diagnostic.line
        ? [
            {
              startLineNumber: diagnostic.line,
              startColumn: diagnostic.column ?? 1,
              endLineNumber: diagnostic.endLine ?? diagnostic.line,
              endColumn: Math.max((diagnostic.endColumn ?? (diagnostic.column ?? 1) + 1), (diagnostic.column ?? 1) + 1),
              message: diagnostic.message ?? 'invalid TOML',
              severity: r.monaco.MarkerSeverity.Error,
            },
          ]
        : []
    r.monaco.editor.setModelMarkers(model, `workbench-${kind}`, markers)
  }, [diagnostic, kind])

  return (
    <div className={fill ? 'wb-toml fill' : 'wb-toml'} style={height ? { height } : undefined}>
      <MonacoEditor
        language="toml"
        theme={theme === 'light' ? 'workbench-light' : 'workbench-dark'}
        value={value}
        onChange={(v) => onChange?.(v ?? '')}
        onMount={(editor, monaco) => {
          refs.current = { editor, monaco }
          // An action, not `addCommand`: its keybinding applies only while this
          // editor has focus, and it goes away with the editor. `addCommand`
          // registers a global Ctrl+S that outlives the Settings tab and would
          // save an abandoned config draft from any other editor.
          const save = editor.addAction({
            id: 'workbench.saveToml',
            label: 'Save and Apply',
            keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS],
            run: () => saveRef.current?.(),
          })
          editor.onDidDispose(() => save.dispose())
        }}
        options={{
          readOnly,
          fontSize,
          fontFamily: "'JetBrains Mono Variable', 'JetBrains Mono', monospace",
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          lineNumbersMinChars: 3,
          renderLineHighlight: readOnly ? 'none' : 'line',
          tabSize: 2,
          automaticLayout: true,
          wordWrap: 'on',
          fixedOverflowWidgets: true,
        }}
      />
    </div>
  )
}

/** One line under an editor: checking… / valid / the parse error. */
export function DiagnosticLine({ diag, checking }: { diag: Diagnostic | null; checking: boolean }) {
  if (checking) {
    return (
      <div className="wb-toml-status">
        <Spinner size={11} /> Checking…
      </div>
    )
  }
  if (!diag) return <div className="wb-toml-status" />
  if (!diag.ok) {
    return (
      <div className="wb-toml-status">
        <span className="wb-danger">
          <XCircle size={12} />
          {diag.line ? `Line ${diag.line}, column ${diag.column}: ` : ''}
          {diag.message}
        </span>
      </div>
    )
  }
  return (
    <div className="wb-toml-status">
      <span className="wb-success">
        <CheckCircle2 size={12} /> Valid
      </span>
      {diag.warnings.length > 0 && (
        <span className="wb-warning" title={diag.warnings.join('\n')}>
          · {diag.warnings.length === 1 ? diag.warnings[0] : `${diag.warnings.length} warnings`}
        </span>
      )}
    </div>
  )
}

/**
 * Form state seeded from server data. When the server value changes (after a
 * save, or an edit in another window) the draft follows it unless the user has
 * unsaved edits. `dirty` compares normalized values.
 */
export function useDraft<T>(saved: T | undefined, normalize: (v: T) => unknown = (v) => v) {
  const [draft, setDraft] = useState<T | null>(null)
  const base = useRef<T | undefined>(undefined)
  const norm = useRef(normalize)
  norm.current = normalize
  useEffect(() => {
    if (saved === undefined) return
    const eq = (a: T, b: T) => JSON.stringify(norm.current(a)) === JSON.stringify(norm.current(b))
    setDraft((d) => (d === null || base.current === undefined || eq(d, base.current) ? saved : d))
    base.current = saved
  }, [saved])
  const dirty = draft !== null && saved !== undefined && JSON.stringify(normalize(draft)) !== JSON.stringify(normalize(saved))
  const reset = () => {
    if (saved !== undefined) setDraft(saved)
  }
  return { draft, setDraft, dirty, reset }
}

/** The current value of `v`, `ms` after it stopped changing. */
export function useDebounced<T>(v: T, ms: number): T {
  const [d, setD] = useState(v)
  useEffect(() => {
    const t = window.setTimeout(() => setD(v), ms)
    return () => window.clearTimeout(t)
  }, [v, ms])
  return d
}
