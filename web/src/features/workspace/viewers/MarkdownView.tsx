// A markdown step: rendered (relative images through the card's grant), with an
// Edit mode (Monaco) that saves against the file's revision. A save over a file
// that changed on disk is a conflict: the draft stays, and the user reloads or
// saves anyway (the previous version is backed up on the server).
// Drafts live in the Workspace store, not in this component (as in Mr. Mak): a
// step switch, an agent opening another step, or a reload keep them.

import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import type { OnMount } from '@monaco-editor/react'
import { AlertTriangle, Eye, Pencil, RotateCcw, Save } from 'lucide-react'
import { ApiError } from '@/api/client'
import { openPanel, toast, toastError } from '@/shell/actions'
import { Button, ErrorBox, Loading, Markdown, MonacoEditor, Toolbar } from '@/ui'
import { useCardText, wk, wsApi, type WorkspaceCard } from '../api'
import { applyCard } from '../actions'
import { basename, fileUrl, headingSlug, isExternalRef, repoLinkPath, resolveRelative } from '../logic'
import { draftKey, isDirty, useWsDrafts } from '../store'
import { OpenButtons } from './basic'

export function MarkdownView({
  card,
  path,
  onOpenPath,
  active,
}: {
  card: WorkspaceCard
  path: string
  /** A relative link to another file of the card: return true when handled (e.g. it is a step). */
  onOpenPath?: (path: string) => boolean
  active?: boolean
}) {
  const qc = useQueryClient()
  const { data, error, isLoading, refetch } = useCardText(card.scope, card.id, path)
  const key = draftKey(card.scope, card.id, path)
  /** Present while the file is in edit mode. */
  const entry = useWsDrafts((s) => s.drafts[key])
  const [conflict, setConflict] = useState(false)
  const [saving, setSaving] = useState(false)
  const saveRef = useRef<() => void>(() => {})
  // The editor is uncontrolled (a controlled value races fast typing); text that
  // replaces the draft goes through `replaceDraft`.
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  // Leaving this file: an editor without changes closes; a changed draft is kept
  // and comes back with the file.
  useEffect(() => {
    setConflict(false)
    return () => {
      const s = useWsDrafts.getState()
      if (s.drafts[key] && !isDirty(s.drafts[key])) s.drop(key)
    }
  }, [key])

  const dirty = isDirty(entry)
  const changedOnDisk = !!entry && !!data && data.revision !== entry.base

  const replaceDraft = (text: string) => {
    editorRef.current?.setValue(text)
    useWsDrafts.getState().setText(key, text)
  }

  const startEdit = () => {
    if (!data) return
    useWsDrafts.getState().put(key, { text: data.text, base: data.revision, baseText: data.text })
    setConflict(false)
  }

  const save = async (force = false) => {
    const current = useWsDrafts.getState().drafts[key]
    if (saving || !current) return
    const text = editorRef.current?.getValue() ?? current.text
    setSaving(true)
    try {
      const r = await wsApi.save(card.scope, card.id, { path, text, revision: current.base, force })
      useWsDrafts.getState().saved(key, r.revision, text)
      setConflict(false)
      qc.setQueryData(wk.content(card.scope, card.id, path), { path, text, revision: r.revision, size: r.size, editable: true })
      const fresh = await wsApi.card(card.scope, card.id).catch(() => null)
      if (fresh) applyCard(qc, fresh)
      toast('success', `Saved ${basename(path)}`)
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) setConflict(true)
      else toastError(e, 'Could not save')
    } finally {
      setSaving(false)
    }
  }
  saveRef.current = () => void save(false)

  const reloadFromDisk = async () => {
    const r = await refetch()
    if (r.data) {
      replaceDraft(r.data.text)
      useWsDrafts.getState().saved(key, r.data.revision, r.data.text)
      setConflict(false)
    }
  }

  const resolveImage = (src: string) => {
    const rel = resolveRelative(path, src)
    return rel === null ? src : fileUrl(card.base, rel)
  }

  /** `#anchor`: scroll to the heading with that GitHub-style slug. */
  const scrollToHeading = (fragment: string) => {
    let want = fragment
    try {
      want = decodeURIComponent(fragment)
    } catch {
      /* keep it raw */
    }
    const scroller = scrollRef.current
    const target = [...(scroller?.querySelectorAll<HTMLElement>('h1, h2, h3, h4, h5, h6') ?? [])].find((h) => headingSlug(h.textContent ?? '') === want.toLowerCase())
    if (scroller && target) scroller.scrollTop += target.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 8
  }

  // Every link is handled here except external ones: the anchor's own href is
  // relative to Workbench's origin, where it would open a second copy of the app.
  const onLinkClick = (href: string) => {
    const h = href.trim()
    if (isExternalRef(h)) return false
    if (h.startsWith('#')) {
      scrollToHeading(h.slice(1))
      return true
    }
    const rel = resolveRelative(path, h)
    if (rel !== null) {
      if (onOpenPath?.(rel)) return true
      window.open(fileUrl(card.base, rel), '_blank', 'noopener,noreferrer')
      return true
    }
    // Out of the card folder: a repository card's link can land in its project.
    const inProject = repoLinkPath(card, path, h)
    if (inProject) {
      const projectId = card.scope
      if (/\.(md|markdown)$/i.test(inProject)) {
        openPanel({ kind: 'markdown', id: `markdown:${projectId}:${inProject}`, title: basename(inProject), params: { projectId, path: inProject } })
      } else {
        openPanel({ kind: 'editor', id: `editor:${projectId}:${inProject}`, title: basename(inProject), params: { projectId, path: inProject } })
      }
      return true
    }
    toast('info', `“${h}” points outside the card folder`)
    return true
  }

  if (error) return <ErrorBox error={error} onRetry={() => void refetch()} />
  if (isLoading || !data) return <Loading />

  return (
    <div className="wb-fill">
      <Toolbar>
        <span className="wb-small wb-muted wb-ellipsis" style={{ padding: '0 4px' }}>
          {path}
        </span>
        {dirty && <span className="wb-badge accent">edited</span>}
        <span className="spacer" />
        {entry ? (
          <>
            <Button
              size="small"
              variant="ghost"
              icon={Eye}
              onClick={() => useWsDrafts.getState().drop(key)}
              disabled={dirty}
              title={dirty ? 'Save or discard first' : 'Back to the rendered view'}
            >
              View
            </Button>
            {dirty && (
              <Button size="small" variant="ghost" icon={RotateCcw} onClick={() => replaceDraft(entry.baseText)}>
                Discard
              </Button>
            )}
            <Button size="small" variant="primary" icon={Save} loading={saving} disabled={!dirty && !changedOnDisk} onClick={() => void save(false)}>
              Save
            </Button>
          </>
        ) : (
          data.editable && (
            <Button size="small" icon={Pencil} onClick={startEdit}>
              Edit
            </Button>
          )
        )}
        <OpenButtons url={fileUrl(card.base, path)} name={basename(path)} />
      </Toolbar>
      {entry && (conflict || changedOnDisk) && (
        <div className="ws-conflict" role="alert">
          <AlertTriangle size={15} />
          <span className="wb-grow">
            {conflict ? 'The file changed on disk since you started editing. Your draft is kept.' : 'The file changed on disk (an agent may have edited it).'}
          </span>
          <Button size="small" onClick={() => void reloadFromDisk()}>
            Load the disk version
          </Button>
          <Button size="small" variant="danger" onClick={() => void save(true)}>
            Save mine anyway
          </Button>
        </div>
      )}
      {entry ? (
        <div className="ws-md-editor">
          <MonacoEditor
            key={key}
            language="markdown"
            defaultValue={entry.text}
            onChange={(v) => useWsDrafts.getState().setText(key, v ?? '')}
            options={{ wordWrap: 'on', minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }}
            onMount={(editor, monaco) => {
              editorRef.current = editor
              editor.onDidDispose(() => {
                if (editorRef.current === editor) editorRef.current = null
              })
              editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => saveRef.current())
              if (active !== false) editor.focus()
            }}
          />
        </div>
      ) : (
        <div className="wb-scroll ws-md-scroll" ref={scrollRef}>
          <div className="ws-md-page">
            <Markdown text={data.text} resolveImage={resolveImage} onLinkClick={onLinkClick} />
          </div>
        </div>
      )}
    </div>
  )
}
