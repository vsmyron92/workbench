// The `confluence` panel: view a page, edit it (rich editor or storage source), see
// its history and comments. Params: {pageId, mode?: 'view'|'edit', projectId?}.
// Unsaved edits are kept as a per-page draft in localStorage until saved or discarded.

import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  AlertTriangle,
  ArrowLeft,
  Bot,
  ChevronRight,
  Code2,
  Copy,
  CopyPlus,
  ExternalLink,
  Eye,
  EyeOff,
  FilePlus,
  FolderInput,
  History as HistoryIcon,
  MessageSquare,
  MoreHorizontal,
  Paperclip,
  Pencil,
  RefreshCw,
  Save,
  Trash2,
  Type,
} from 'lucide-react'
import { ApiError } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { useUi } from '@/state/store'
import { monacoThemeName } from '@/theme/palette'
import { Button, EmptyState, ErrorBox, IconButton, Loading, MonacoEditor, showMenuAt, Spacer, Tabs, TimeAgo, Toolbar } from '@/ui'
import { ConfluenceIcon } from '@/ui/brand'
import { confluenceApi, qk, useComments, useCommentStates, usePage, useWatching, type Page, type UpdateOut } from '../api'
import { StaleNotice } from '../StaleNotice'
import { useAtlassianUi, usePrefs } from '../state'
import { storageToDoc } from '../storage/convert'
import { XmlError } from '../storage/xml'
import { askAgentAboutPage, copyText, openConfluencePage, openExternal } from './actions'
import { AttachmentsPane } from './AttachmentsPane'
import { CommentsPane } from './CommentsPane'
import { History } from './History'
import { Labels } from './Labels'
import { trashPage } from './PageOps'
import { PageView, type PageViewHandle } from './PageView'
import type { Anchor } from './selection'
import type { RichEditorHandle } from './RichEditor'
import { SaveDialog, type SaveRequest } from './SaveDialog'
import { SetupHint } from './ToolWindow'

const RichEditor = lazy(() => import('./RichEditor'))

export interface ConfluenceParams {
  pageId: string
  mode?: 'view' | 'edit'
  projectId?: string
}

interface EditSession {
  base: number
  baseStorage: string
  baseTitle: string
  title: string
  kind: 'rich' | 'source'
  storage: string
  /** The unchanged document as the editor writes it (no-op detection). */
  baseline: string
  richKey: number
  /** The rich editor had to adjust structure it cannot represent exactly. */
  adjusted: boolean
}

interface Draft {
  base: number
  title: string
  storage: string
  at: number
}

const draftKey = (id: string) => `wb.cf.draft.${id}`

function loadDraft(id: string): Draft | null {
  try {
    const raw = localStorage.getItem(draftKey(id))
    return raw ? (JSON.parse(raw) as Draft) : null
  } catch {
    return null
  }
}

function storeDraft(id: string, d: Draft) {
  try {
    localStorage.setItem(draftKey(id), JSON.stringify(d))
  } catch {
    /* quota: drafts are best effort */
  }
}

function clearDraft(id: string) {
  try {
    localStorage.removeItem(draftKey(id))
  } catch {
    /* ignore */
  }
}

function Crumbs({ page }: { page: Page }) {
  return (
    <div className="cf-crumbs">
      {page.spaceName && (
        <>
          <span className="wb-subtle" style={{ padding: '0 4px', whiteSpace: 'nowrap' }}>
            {page.spaceName}
          </span>
          <ChevronRight size={12} className="sep" />
        </>
      )}
      {page.ancestors.map((a) => (
        <span key={a.id} className="wb-row" style={{ gap: 2, minWidth: 0, flexShrink: 1 }}>
          <button onClick={() => a.type !== 'folder' && openConfluencePage(a.id, a.title)} title={a.title} disabled={a.type === 'folder'}>
            {a.title}
          </button>
          <ChevronRight size={12} className="sep" />
        </span>
      ))}
      <span className="current" title={page.title}>
        {page.title}
      </span>
    </div>
  )
}

function Meta({ page, projectId }: { page: Page; projectId: string | null }) {
  return (
    <div className="cf-meta">
      <span>v{page.version.number}</span>
      <span>
        Updated <TimeAgo time={page.version.createdAt} />
        {page.version.authorName ? ` by ${page.version.authorName}` : ''}
      </span>
      {page.version.message && (
        <span className="wb-ellipsis" style={{ maxWidth: 420 }} title={page.version.message}>
          “{page.version.message}”
        </span>
      )}
      <Labels projectId={projectId} page={page} readOnly={page.status !== 'current' || page.historical} />
    </div>
  )
}

export function ConfluencePanel({ params, setParams, setTitle, active }: PanelProps<ConfluenceParams>) {
  const qc = useQueryClient()
  const uiProject = useUi((s) => s.projectId)
  const projectId = params.projectId ?? uiProject
  const pageId = String(params.pageId ?? '')
  const q = usePage(projectId, pageId)
  const page = q.data
  const addRecent = usePrefs((s) => s.addRecent)
  const sideDefault = usePrefs((s) => s.sidePane ?? (s.commentsOpen ? 'comments' : null))
  const setSideDefault = usePrefs((s) => s.setSidePane)
  const [mode, setMode] = useState<'view' | 'edit' | 'history'>('view')
  const [session, setSession] = useState<EditSession | null>(null)
  const [saveReq, setSaveReq] = useState<SaveRequest | null>(null)
  const [side, setSideState] = useState<'comments' | 'attachments' | null>(sideDefault)
  const [activeRef, setActiveRef] = useState<string | null>(null)
  const [draft, setDraft] = useState<Anchor | null>(null)
  // Comment states colour the highlights: the open pane's threads, else a light list
  // (without replies) whenever the page has highlights.
  const comments = useComments(projectId, pageId, side === 'comments')
  const states = useCommentStates(projectId, pageId, side !== 'comments' && !!page?.hasInlineCommentMarkers)
  const watching = useWatching(projectId, pageId, mode === 'view' && !!page)
  const threads = comments.data ?? states.data
  const markers = useMemo(() => {
    const out: Record<string, 'open' | 'resolved'> = {}
    for (const c of threads?.inline ?? []) if (c.markerRef) out[c.markerRef] = c.resolutionStatus === 'resolved' ? 'resolved' : 'open'
    return out
  }, [threads])
  const viewRef = useRef<PageViewHandle>(null)
  const richRef = useRef<RichEditorHandle>(null)
  const draftTimer = useRef<number | undefined>(undefined)

  useEffect(() => {
    if (!page) return
    setTitle(page.title)
    addRecent({ id: page.id, title: page.title, spaceKey: page.spaceKey })
  }, [page?.id, page?.title]) // eslint-disable-line react-hooks/exhaustive-deps

  const refresh = () => {
    qc.invalidateQueries({ queryKey: qk.page(projectId, pageId) })
    qc.invalidateQueries({ queryKey: qk.comments(projectId, pageId) })
    qc.invalidateQueries({ queryKey: qk.versions(projectId, pageId) })
  }

  const startEdit = useCallback(
    async (kind: 'rich' | 'source') => {
      if (!page) return
      if (page.status === 'archived') {
        toast('warning', 'Archived pages are read-only; restore the page in Confluence to edit it')
        return
      }
      let storage = page.storage
      let title = page.title
      let base = page.version.number
      let baseStorage = page.storage
      const draft = loadDraft(page.id)
      if (draft && (draft.storage !== page.storage || draft.title !== page.title)) {
        const restore = await confirmDialog({
          title: 'Restore your unsaved draft?',
          message:
            `You have unsaved changes to this page from ${new Date(draft.at).toLocaleString()}` +
            (draft.base !== page.version.number ? `, based on version ${draft.base} (the page is now version ${page.version.number}; saving will show the conflict).` : '.'),
          confirmLabel: 'Restore draft',
        })
        if (restore) {
          storage = draft.storage
          title = draft.title
          if (draft.base !== page.version.number) {
            base = draft.base
            baseStorage = page.storage
          }
        } else clearDraft(page.id)
      }
      if (kind === 'rich') {
        try {
          storageToDoc(storage)
        } catch (e) {
          toast('warning', `The rich editor cannot read this page (${e instanceof Error ? e.message : String(e)}); opening the source editor`)
          kind = 'source'
        }
      }
      setSession({ base, baseStorage, baseTitle: page.title, title, kind, storage, baseline: page.storage, richKey: 1, adjusted: false })
      setMode('edit')
    },
    [page],
  )

  // Panels opened with mode: 'edit' (e.g. after creating a page) start editing once.
  // dockview merges parameter updates, so the flag is cleared explicitly.
  useEffect(() => {
    if (params.mode === 'edit' && page && !session) {
      void startEdit('rich')
      setParams({ ...params, mode: undefined })
    }
  }, [params, page, session, startEdit, setParams])

  const dirty = !!session && (session.storage !== session.baseline || session.title !== session.baseTitle)

  const updateSession = (patch: Partial<EditSession>) => {
    setSession((s) => {
      if (!s) return s
      const next = { ...s, ...patch }
      window.clearTimeout(draftTimer.current)
      draftTimer.current = window.setTimeout(() => {
        if (next.storage !== next.baseline || next.title !== next.baseTitle) storeDraft(pageId, { base: next.base, title: next.title, storage: next.storage, at: Date.now() })
        else clearDraft(pageId)
      }, 1200)
      return next
    })
  }

  const currentStorage = () => (session?.kind === 'rich' ? richRef.current?.getStorage() ?? session.storage : session?.storage ?? '')

  const switchKind = (kind: 'rich' | 'source') => {
    if (!session || session.kind === kind) return
    const storage = currentStorage()
    if (kind === 'rich') {
      try {
        storageToDoc(storage)
      } catch (e) {
        toast('error', e instanceof XmlError ? `Fix the source first: ${e.message}` : String(e))
        return
      }
    }
    updateSession({ kind, storage, richKey: session.richKey + 1 })
  }

  const exitEdit = () => {
    window.clearTimeout(draftTimer.current)
    clearDraft(pageId)
    setSession(null)
    setSaveReq(null)
    setMode('view')
  }

  const cancel = async () => {
    if (dirty && !(await confirmDialog({ title: 'Discard your changes?', message: 'Your edits to this page will be lost.', confirmLabel: 'Discard', danger: true }))) return
    exitEdit()
  }

  const openSave = () => {
    if (!session) return
    const storage = currentStorage()
    if (storage === session.baseline && session.title === session.baseTitle) {
      toast('info', 'No changes to save')
      return
    }
    setSaveReq({ base: session.base, baseStorage: session.baseStorage, title: session.title.trim() || session.baseTitle, storage })
  }

  const onSaved = (out: UpdateOut) => {
    exitEdit()
    toast('success', out.unchanged ? 'No changes: the page is unchanged' : `Saved “${out.title}” as version ${out.version}`)
    refresh()
    qc.invalidateQueries({ predicate: (qq) => (qq.queryKey as unknown[])[0] === 'confluence' && ['children', 'roots', 'byIds'].includes(String((qq.queryKey as unknown[])[1])) })
  }

  // Ctrl/Cmd+S saves while this panel is the active editor (focus may be anywhere).
  const openSaveRef = useRef(openSave)
  openSaveRef.current = openSave
  useEffect(() => {
    if (mode !== 'edit' || !active) return
    const onKey = (e: globalThis.KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 's') {
        e.preventDefault()
        if (!document.querySelector('.wb-modal')) openSaveRef.current()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [mode, active])

  const setSide = (v: 'comments' | 'attachments' | null) => {
    setSideState(v)
    setSideDefault(v)
    if (v !== 'comments') {
      setDraft(null)
      viewRef.current?.holdSelection(false)
    }
  }
  const toggleSide = (v: 'comments' | 'attachments') => setSide(side === v ? null : v)

  const startComment = (anchor: Anchor) => {
    setDraft(anchor)
    setActiveRef(null)
    setSideState('comments')
    setSideDefault('comments')
  }

  // Ctrl+Alt+C comments on the text selected in the page (as in Confluence).
  const startCommentRef = useRef(startComment)
  startCommentRef.current = startComment
  const canComment = !!page && page.status === 'current' && !page.historical
  useEffect(() => {
    if (mode !== 'view' || !active || !canComment) return
    const onKey = (e: globalThis.KeyboardEvent) => {
      // Not while typing: AltGr+C (a letter on some layouts) arrives as Ctrl+Alt+C.
      const target = e.target as HTMLElement | null
      if (target?.closest?.('input, textarea, select, [contenteditable="true"]')) return
      if ((e.ctrlKey || e.metaKey) && e.altKey && !e.shiftKey && e.code === 'KeyC') {
        const a = viewRef.current?.selectionAnchor()
        if (!a) return
        e.preventDefault()
        if ('error' in a) toast('warning', a.error)
        else {
          viewRef.current?.holdSelection(true)
          startCommentRef.current(a)
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [mode, active, canComment])

  const toggleWatch = async () => {
    const now = !!watching.data?.watching
    try {
      await confluenceApi.setWatching(projectId, pageId, !now)
      qc.setQueryData(qk.watch(projectId, pageId), { watching: !now })
      toast('success', now ? 'You no longer watch this page' : 'Watching: Confluence notifies you of changes to this page')
    } catch (e) {
      toastError(e, 'Could not change watching')
    }
  }

  if (!/^\d+$/.test(pageId)) return <EmptyState icon={ConfluenceIcon} title="Not a Confluence page">The page id “{pageId}” is not valid.</EmptyState>
  if (q.isLoading) return <Loading label="Loading page…" />
  // A failed refresh keeps the page (and an edit, comments being written) on screen.
  if (q.error && !page) {
    if (q.error instanceof ApiError && q.error.notConfigured) return <SetupHint error={q.error} />
    return <ErrorBox error={q.error} onRetry={() => q.refetch()} />
  }
  if (!page) return null
  const stale = q.error ? <StaleNotice what="the page" error={q.error} onRetry={() => void q.refetch()} /> : null

  const writable = page.status === 'current' && !page.historical
  const moreMenu = (el: HTMLElement) =>
    showMenuAt(el, [
      { label: 'Copy link', icon: Copy, run: () => void copyText(page.webUrl) },
      { label: 'Copy page id', icon: Copy, run: () => void copyText(page.id, 'Page id') },
      { label: 'Edit in Confluence', icon: ExternalLink, run: () => openExternal(page.editUrl ?? page.webUrl) },
      'separator',
      { label: 'Move…', icon: FolderInput, disabled: !writable, run: () => useAtlassianUi.getState().openPageOp({ kind: 'move', page, projectId }) },
      { label: 'Copy…', icon: CopyPlus, run: () => useAtlassianUi.getState().openPageOp({ kind: 'copy', page, projectId }) },
      {
        label: watching.data ? (watching.data.watching ? 'Stop watching' : 'Watch page') : 'Watch page (checking…)',
        icon: watching.data?.watching ? EyeOff : Eye,
        disabled: !watching.data,
        run: () => void toggleWatch(),
      },
      'separator',
      { label: 'Refresh', icon: RefreshCw, run: refresh },
      'separator',
      { label: 'Move to trash…', icon: Trash2, danger: true, disabled: !writable, run: () => void trashPage(qc, projectId, page) },
    ])

  // ---------------------------------------------------------------- edit mode
  if (mode === 'edit' && session) {
    const titleInput = (
      <input
        className="cf-edit-title"
        value={session.title}
        placeholder="Page title"
        aria-label="Page title"
        onChange={(e) => updateSession({ title: e.target.value })}
      />
    )
    return (
      <div className="cf-page">
        <Toolbar>
          <Pencil size={13} className="wb-muted" />
          <span className="title wb-ellipsis">Editing {page.title}</span>
          <Tabs
            tabs={[
              { id: 'rich', label: <><Type size={13} /> Rich</> },
              { id: 'source', label: <><Code2 size={13} /> Source</> },
            ]}
            value={session.kind}
            onChange={switchKind}
          />
          <Spacer />
          {dirty ? <span className="wb-xs wb-warning">Unsaved changes (draft kept)</span> : <span className="wb-xs wb-subtle">No changes</span>}
          <Button size="small" variant="ghost" onClick={() => void cancel()} style={{ marginLeft: 6 }}>
            Cancel
          </Button>
          <Button size="small" variant="primary" icon={Save} onClick={openSave} title="Save (Ctrl+S)">
            Save…
          </Button>
        </Toolbar>
        {stale}
        {session.base !== page.version.number && (
          <div className="cf-banner">
            <AlertTriangle size={14} className="wb-warning" /> This draft is based on version {session.base}; the page is now version {page.version.number}. Saving will
            show both so you can choose.
          </div>
        )}
        {session.adjusted && session.kind === 'rich' && (
          <div className="cf-banner">
            <AlertTriangle size={14} className="wb-warning" /> The rich editor adjusted some structure on this page. Review the changes before saving, or use Source to edit
            the XHTML exactly.
          </div>
        )}
        {page.hasInlineCommentMarkers && (
          <div className="cf-banner info">
            <MessageSquare size={14} /> {page.inlineMarkerRefs.length} inline comment{page.inlineMarkerRefs.length === 1 ? ' is' : 's are'} anchored in the text
            (highlighted). Keep those passages so the comments stay attached.
          </div>
        )}
        <div className="cf-body">
          {session.kind === 'rich' ? (
            <Suspense fallback={<Loading label="Loading editor…" />}>
              <RichEditor
                key={session.richKey}
                ref={richRef}
                storage={session.storage}
                pageId={page.id}
                projectId={projectId}
                spaceKey={page.spaceKey}
                users={page.users}
                header={titleInput}
                onChange={(storage) => updateSession({ storage })}
                onReady={(normalized, adjusted) =>
                  // Only the first load defines "unchanged"; later remounts keep it.
                  setSession((s) => (s ? { ...s, baseline: s.richKey === 1 && s.storage === page.storage ? normalized : s.baseline, adjusted } : s))
                }
                onError={(e) => {
                  toast('warning', `The rich editor could not load this page: ${e instanceof Error ? e.message : String(e)}`)
                  setSession((s) => (s ? { ...s, kind: 'source' } : s))
                }}
              />
            </Suspense>
          ) : (
            <div className="cf-edit">
              <div style={{ padding: '12px 16px 0' }}>{titleInput}</div>
              <div className="cf-source">
                <MonacoEditor
                  height="100%"
                  theme={monacoThemeName()}
                  language="xml"
                  path={`confluence://${page.id}/storage.xml`}
                  value={session.storage}
                  onChange={(v) => updateSession({ storage: v ?? '' })}
                  options={{ wordWrap: 'on', automaticLayout: true, minimap: { enabled: false }, fontSize: 12, scrollBeyondLastLine: false, tabSize: 2 }}
                />
              </div>
            </div>
          )}
        </div>
        {saveReq && (
          <SaveDialog
            projectId={projectId}
            page={page}
            req={saveReq}
            onClose={() => setSaveReq(null)}
            onSaved={onSaved}
            onDiscard={() => {
              exitEdit()
              refresh()
            }}
          />
        )}
      </div>
    )
  }

  // ---------------------------------------------------------------- history
  if (mode === 'history') {
    return (
      <div className="cf-page">
        <Toolbar>
          <Button size="small" variant="ghost" icon={ArrowLeft} onClick={() => setMode('view')}>
            Page
          </Button>
          <Crumbs page={page} />
          <HistoryIcon size={13} className="wb-muted" />
          <span className="wb-small wb-muted">History</span>
        </Toolbar>
        {stale}
        <div className="cf-body">
          <History projectId={projectId} page={page} />
        </div>
      </div>
    )
  }

  // ---------------------------------------------------------------- view
  const readOnly = page.status !== 'current' || page.historical
  return (
    <div className="cf-page">
      <Toolbar>
        <Crumbs page={page} />
        {q.isFetching && <span className="wb-spinner" style={{ width: 11, height: 11 }} />}
        <IconButton icon={Pencil} label={readOnly ? 'Archived pages are read-only' : 'Edit'} disabled={readOnly} onClick={() => void startEdit('rich')} />
        <IconButton icon={Code2} label="Edit source (storage XHTML)" disabled={readOnly} onClick={() => void startEdit('source')} />
        <IconButton icon={HistoryIcon} label="History" onClick={() => setMode('history')} />
        <IconButton icon={MessageSquare} label="Comments (select text to comment on it: Ctrl+Alt+C)" active={side === 'comments'} onClick={() => toggleSide('comments')} />
        <IconButton icon={Paperclip} label="Attachments" active={side === 'attachments'} onClick={() => toggleSide('attachments')} />
        <IconButton
          icon={FilePlus}
          label="New child page…"
          onClick={() => useAtlassianUi.getState().openNewPage({ parentId: page.id, parentTitle: page.title, spaceId: page.spaceId })}
        />
        <IconButton icon={Bot} label="Ask agent about this page…" onClick={() => void askAgentAboutPage(projectId, { id: page.id, title: page.title, version: page.version.number, webUrl: page.webUrl })} />
        <IconButton icon={ExternalLink} label="Open in browser" onClick={() => openExternal(page.webUrl)} />
        <IconButton icon={MoreHorizontal} label="More" onClick={(e) => moreMenu(e.currentTarget)} />
      </Toolbar>
      {stale}
      {readOnly && (
        <div className="cf-banner info">
          <AlertTriangle size={14} /> This page is archived.
        </div>
      )}
      <div className="cf-body">
        <div className="cf-main">
          <div className="cf-doc">
            <h1 className="cf-title">{page.title}</h1>
            <Meta page={page} projectId={projectId} />
            <PageView
              ref={viewRef}
              html={page.html}
              pageId={page.id}
              markers={markers}
              activeRef={activeRef}
              onComment={readOnly ? undefined : startComment}
              onMarkerClick={(ref) => {
                setActiveRef(ref)
                setSide('comments')
              }}
            />
            {!page.html.trim() && <EmptyState title="This page is empty" />}
          </div>
        </div>
        {side === 'comments' && (
          <CommentsPane
            projectId={projectId}
            page={page}
            activeRef={activeRef}
            draft={draft}
            onDraftDone={(markerRef) => {
              setDraft(null)
              viewRef.current?.holdSelection(false)
              if (markerRef) setActiveRef(markerRef)
            }}
            onActivate={setActiveRef}
            onJumpToMarker={(ref) => viewRef.current?.flashMarker(ref) ?? false}
            onClose={() => setSide(null)}
          />
        )}
        {side === 'attachments' && <AttachmentsPane projectId={projectId} page={page} onClose={() => setSide(null)} />}
      </div>
    </div>
  )
}
