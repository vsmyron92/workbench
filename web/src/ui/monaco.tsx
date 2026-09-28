// Lazy Monaco wrappers. The ~1 MB editor chunk loads the first time any feature
// shows an editor or diff. Props are those of @monaco-editor/react.

import { lazy, Suspense, type ComponentProps } from 'react'
import type { DiffEditor as DiffEditorT, DiffOnMount, Editor as EditorT } from '@monaco-editor/react'

function Loading({ label }: { label: string }) {
  return (
    <div className="wb-empty">
      <span className="wb-spinner" />
      <span>{label}</span>
    </div>
  )
}

const LazyEditor = lazy(async () => {
  await import('@/lib/monacoSetup')
  const m = await import('@monaco-editor/react')
  return { default: m.Editor }
})

const LazyDiffEditor = lazy(async () => {
  await import('@/lib/monacoSetup')
  const m = await import('@monaco-editor/react')
  return { default: m.DiffEditor }
})

/**
 * Monaco defines its theme colours as `--vscode-*` variables on `.monaco-editor`,
 * `.monaco-diff-editor` and `.monaco-component`. Context menus and other shadow-DOM
 * widgets are attached to the editor's container, next to `.monaco-editor` rather
 * than inside it, so without this class they inherit no colours (a transparent menu).
 */
function containerClass(className: string | undefined) {
  return className ? `monaco-component ${className}` : 'monaco-component'
}

export function MonacoEditor(props: ComponentProps<typeof EditorT>) {
  return (
    <Suspense fallback={<Loading label="Loading editor…" />}>
      <LazyEditor theme="workbench-dark" loading={<Loading label="Loading editor…" />} {...props} className={containerClass(props.className)} />
    </Suspense>
  )
}

type DiffEditorInstance = Parameters<DiffOnMount>[0]
type DiffModels = ReturnType<DiffEditorInstance['getModel']>

/** Dispose models once no editor shows them (a later tick: after the widget let go). */
function releaseLater(models: DiffModels) {
  setTimeout(() => {
    for (const m of [models?.original, models?.modified]) {
      if (m && !m.isDisposed() && !m.isAttachedToEditor()) m.dispose()
    }
  }, 0)
}

/**
 * Monaco (0.57) keeps every disposed diff editor alive, DOM and all: its overview
 * ruler stays subscribed to theme changes. Switching the ruler off just before
 * dispose releases it. The diff editor's own onDidDispose never fires, so the hook
 * goes on dispose itself (@monaco-editor/react calls it on unmount); `after` runs
 * once the widget is gone.
 */
function onDispose(ed: DiffEditorInstance, after: () => void) {
  const dispose = ed.dispose.bind(ed)
  ed.dispose = () => {
    try {
      ed.updateOptions({ renderOverviewRuler: false })
    } catch {
      // Disposing anyway.
    }
    dispose()
    after()
  }
}

export function MonacoDiffEditor(props: ComponentProps<typeof DiffEditorT>) {
  const { onMount, keepCurrentOriginalModel, keepCurrentModifiedModel } = props
  // @monaco-editor/react disposes the models *before* the diff widget, which throws
  // "TextModel got disposed before DiffEditorWidget model got reset" on every unmount.
  // Unless the caller manages its models itself, keep them and dispose them after.
  const managed = keepCurrentOriginalModel === undefined && keepCurrentModifiedModel === undefined
  const handleMount: DiffOnMount = (editor, monaco) => {
    let models = managed ? editor.getModel() : null
    if (managed) {
      editor.onDidChangeModel(() => {
        const previous = models
        models = editor.getModel()
        releaseLater(previous)
      })
    }
    onDispose(editor, () => {
      if (managed) releaseLater(models)
    })
    onMount?.(editor, monaco)
  }
  return (
    <Suspense fallback={<Loading label="Loading diff…" />}>
      <LazyDiffEditor
        theme="workbench-dark"
        loading={<Loading label="Loading diff…" />}
        {...props}
        className={containerClass(props.className)}
        keepCurrentOriginalModel={managed || keepCurrentOriginalModel}
        keepCurrentModifiedModel={managed || keepCurrentModifiedModel}
        onMount={handleMount}
      />
    </Suspense>
  )
}
