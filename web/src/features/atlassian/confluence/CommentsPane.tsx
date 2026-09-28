// Page comments beside the page. Page (footer) comments: threads with replies, edit
// and delete of your own, a composer. Inline comments: the passage each is anchored
// to (click to find it in the page), resolve / reopen, replies, edit and delete, and a
// new comment on the text selected in the page. Inline comments whose highlight is no
// longer in the page are flagged as detached. Composers take markdown, with @ to
// mention someone; their text is a draft (`drafts.ts`) until posted or cancelled. An
// edit saves against the version it started from: when the comment moved meanwhile,
// the newer text is shown and the user overwrites it or drops their edit.

import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Check, Copy, ExternalLink, MessageSquare, MessageSquarePlus, MoreHorizontal, Pencil, Reply, RotateCcw, Trash2, X } from 'lucide-react'
import { ApiError } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Badge, Button, Checkbox, EmptyState, ErrorBox, IconButton, Loading, showMenuAt, Spacer, Spinner, Tabs, TextArea, TimeAgo, Toolbar } from '@/ui'
import { confluenceApi, qk, useComments, type Comment, type Page } from '../api'
import { initials } from '../links'
import { StaleNotice } from '../StaleNotice'
import { useAtlassianStatus } from '../state'
import { copyText } from './actions'
import { draftKey, inlineDraftKey, pendingInline, useCommentDrafts } from './drafts'
import { sanitize } from './PageView'
import { useUserSearch } from './people'
import type { Anchor } from './selection'

const MENTION_BEFORE_CARET = /(^|[\s(])@([\p{L}\p{N}._'-]{1,30})$/u

/**
 * A markdown text area; typing @name offers people and inserts `[@Name](mention:id)`.
 * The text lives in the draft store under `draftKey` (it outlives this component) and
 * is dropped when `onSubmit` succeeds or the composer is cancelled.
 */
function Composer({
  projectId,
  draftKey: key,
  anchor,
  placeholder,
  submitLabel = 'Comment',
  onSubmit,
  onCancel,
  autoFocus,
}: {
  projectId: string | null
  draftKey: string
  /** New inline comments: kept with the draft, so it can be posted later. */
  anchor?: Anchor
  placeholder: string
  submitLabel?: string
  onSubmit: (markdown: string) => Promise<boolean>
  onCancel?: () => void
  autoFocus?: boolean
}) {
  const stored = useCommentDrafts((s) => s.drafts[key])
  const text = stored?.text ?? ''
  // An empty new comment or reply is no draft; an edit keeps its entry (and version) while open.
  const setText = (t: string) => {
    const { put, clear } = useCommentDrafts.getState()
    if (t || stored?.version !== undefined) put(key, anchor ? { text: t, anchor } : { text: t })
    else clear(key)
  }
  const cancel = onCancel
    ? () => {
        useCommentDrafts.getState().clear(key)
        onCancel()
      }
    : undefined
  const [busy, setBusy] = useState(false)
  const [mention, setMention] = useState<{ query: string; start: number } | null>(null)
  const [active, setActive] = useState(0)
  const ref = useRef<HTMLTextAreaElement>(null)
  const people = useUserSearch(projectId, mention?.query ?? null)
  const hits = mention ? (people.data ?? []) : []
  useEffect(() => setActive(0), [people.data])

  const submit = async () => {
    if (!text.trim() || busy) return
    setBusy(true)
    const ok = await onSubmit(text)
    setBusy(false)
    if (ok) useCommentDrafts.getState().clear(key)
  }

  const track = (value: string, caret: number) => {
    const m = MENTION_BEFORE_CARET.exec(value.slice(0, caret))
    setMention(m ? { query: m[2], start: caret - m[2].length - 1 } : null)
  }

  const pick = (i: number) => {
    const u = hits[i]
    const el = ref.current
    if (!u || !mention || !el) return
    const caret = el.selectionStart
    const insert = `[@${u.displayName.replace(/[[\]]/g, '')}](mention:${u.accountId}) `
    const next = text.slice(0, mention.start) + insert + text.slice(caret)
    setText(next)
    setMention(null)
    const pos = mention.start + insert.length
    window.requestAnimationFrame(() => {
      el.focus()
      el.setSelectionRange(pos, pos)
    })
  }

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (mention && hits.length) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        setActive((a) => (a + (e.key === 'ArrowDown' ? 1 : -1) + hits.length) % hits.length)
        return
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault()
        pick(active)
        return
      }
    }
    if (e.key === 'Escape') {
      if (mention) {
        e.stopPropagation()
        setMention(null)
      } else cancel?.()
      return
    }
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) void submit()
  }

  return (
    <div className="cf-compose">
      <div className="cf-compose-field">
        <TextArea
          ref={ref}
          value={text}
          rows={3}
          autoFocus={autoFocus}
          placeholder={placeholder}
          onChange={(e) => {
            setText(e.target.value)
            track(e.target.value, e.target.selectionStart)
          }}
          onKeyDown={onKeyDown}
          onClick={(e) => track(text, e.currentTarget.selectionStart)}
          onBlur={() => window.setTimeout(() => setMention(null), 150)}
        />
        {mention && (
          <div className="cf-suggest inline" role="listbox" onMouseDown={(e) => e.preventDefault()}>
            {hits.map((u, i) => (
              <div key={u.accountId} role="option" aria-selected={i === active} className={i === active ? 'item active' : 'item'} onMouseEnter={() => setActive(i)} onClick={() => pick(i)}>
                <span className="atl-avatar">{initials(u.displayName)}</span>
                <span className="wb-ellipsis">{u.displayName}</span>
              </div>
            ))}
            {!hits.length && <div className="empty">{people.isFetching ? <Spinner size={11} /> : `No one named “${mention.query}”`}</div>}
          </div>
        )}
      </div>
      <div className="wb-row">
        <span className="wb-xs wb-subtle">Markdown · @ to mention · Ctrl+Enter</span>
        <Spacer />
        {cancel && (
          <Button size="small" variant="ghost" onClick={cancel}>
            Cancel
          </Button>
        )}
        <Button size="small" variant="primary" loading={busy} disabled={!text.trim()} onClick={() => void submit()}>
          {submitLabel}
        </Button>
      </div>
    </div>
  )
}

interface CardProps {
  c: Comment
  projectId: string | null
  pageId: string
  me: string | null
  /** The comments are being fetched again (after a conflict: their newer text is on its way). */
  refreshing?: boolean
  /** Top-level inline comment: resolve / reopen, quote. */
  thread?: boolean
  detached?: boolean
  active?: boolean
  onQuote?: () => void
  onReply?: (markdown: string) => Promise<boolean>
  onChanged: () => void
}

function CommentCard({ c, projectId, pageId, me, refreshing, thread, detached, active, onQuote, onReply, onChanged }: CardProps) {
  const editKey = draftKey(projectId, pageId, 'edit', c.id)
  const replyKey = draftKey(projectId, pageId, 'reply', c.id)
  // Open editors and replies come back with their text after the card was unmounted.
  const edit = useCommentDrafts((s) => s.drafts[editKey])
  const [replying, setReplying] = useState(() => !!useCommentDrafts.getState().drafts[replyKey]?.text)
  const editing = !!edit
  /** The version the edit started from (sent with the save). */
  const editFrom = edit?.version ?? c.version
  const [conflict, setConflict] = useState(false)
  const moved = editing && editFrom !== c.version
  const [busy, setBusy] = useState<string | null>(null)
  const el = useRef<HTMLDivElement>(null)
  const resolved = c.resolutionStatus === 'resolved'
  const dangling = c.resolutionStatus === 'dangling'
  const mine = !!me && c.authorId === me
  const kind = c.kind

  useEffect(() => {
    if (active) el.current?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [active])

  const run = async (what: string, fn: () => Promise<unknown>, done: string) => {
    setBusy(what)
    try {
      await fn()
      toast('success', done)
      onChanged()
      return true
    } catch (e) {
      toastError(e, 'Confluence refused the change')
      return false
    } finally {
      setBusy(null)
    }
  }

  const resolve = (resolved: boolean) =>
    void run('resolve', () => confluenceApi.updateComment(projectId, 'inline', c.id, { resolved }), resolved ? 'Comment resolved' : 'Comment reopened')

  const remove = async () => {
    const ok = await confirmDialog({
      title: 'Delete this comment?',
      message: c.replies.length ? `Its ${c.replies.length} repl${c.replies.length === 1 ? 'y goes' : 'ies go'} with it. This cannot be undone.` : 'This cannot be undone.',
      confirmLabel: 'Delete',
      danger: true,
    })
    if (ok) void run('delete', () => confluenceApi.deleteComment(projectId, kind, c.id), 'Comment deleted')
  }

  const save = async (markdown: string, version: number) => {
    setBusy('edit')
    try {
      await confluenceApi.updateComment(projectId, kind, c.id, { markdown, version })
      setConflict(false)
      toast('success', 'Comment updated')
      onChanged()
      return true
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        // Someone else saved first: fetch their version and let the user choose.
        setConflict(true)
        onChanged()
        return false
      }
      toastError(e, 'Confluence refused the change')
      return false
    } finally {
      setBusy(null)
    }
  }

  const overwrite = async () => {
    const text = useCommentDrafts.getState().drafts[editKey]?.text ?? ''
    if (!text.trim()) return
    // Rebase the edit on the version now shown; a newer one still gets a 409.
    useCommentDrafts.getState().put(editKey, { version: c.version })
    if (await save(text, c.version)) useCommentDrafts.getState().clear(editKey)
  }

  const discardMine = () => {
    useCommentDrafts.getState().clear(editKey)
    setConflict(false)
  }

  const menu = (target: HTMLElement) =>
    showMenuAt(target, [
      { label: 'Edit', icon: Pencil, disabled: !mine || editing, run: () => useCommentDrafts.getState().put(editKey, { text: c.markdown, version: c.version }) },
      { label: 'Copy link', icon: Copy, disabled: !c.webUrl, run: () => c.webUrl && void copyText(c.webUrl) },
      { label: 'Open in Confluence', icon: ExternalLink, disabled: !c.webUrl, run: () => c.webUrl && window.open(c.webUrl, '_blank', 'noopener,noreferrer') },
      'separator',
      { label: 'Delete…', icon: Trash2, danger: true, disabled: !mine, run: () => void remove() },
    ])

  return (
    <div ref={el} className={['cf-comment', resolved && 'resolved', active && 'active'].filter(Boolean).join(' ')}>
      <div className="cf-comment-head">
        <span className="atl-avatar">{initials(c.authorName)}</span>
        <span className="who wb-ellipsis">{c.authorName ?? 'Someone'}</span>
        <span className="wb-xs wb-subtle">
          <TimeAgo time={c.createdAt} />
        </span>
        <Spacer />
        {resolved && (
          <Badge tone="success" title={c.resolvedBy ? `Resolved by ${c.resolvedBy}` : undefined}>
            resolved
          </Badge>
        )}
        {detached && !resolved && (
          <Badge tone="warning" title="The highlighted text this comment was made on is no longer in the page">
            detached
          </Badge>
        )}
        {busy && <Spinner size={11} />}
        {thread &&
          (resolved ? (
            <IconButton size="small" icon={RotateCcw} label="Reopen" disabled={!!busy || dangling} onClick={() => resolve(false)} />
          ) : (
            <IconButton size="small" icon={Check} label={dangling ? 'Detached comments cannot be resolved' : 'Resolve'} disabled={!!busy || dangling} onClick={() => resolve(true)} />
          ))}
        <IconButton size="small" icon={MoreHorizontal} label="More" onClick={(e) => menu(e.currentTarget)} />
      </div>
      {c.selection && (
        <div className="cf-comment-quote" onClick={onQuote} title="Show in page">
          {c.selection}
        </div>
      )}
      {editing ? (
        <>
          {(moved || conflict) && (
            <div className="cf-conflict" role="alert">
              <div className="wb-row wb-xs">
                <AlertTriangle size={12} className="wb-warning" style={{ flex: 'none' }} />
                <span>
                  {moved
                    ? `This comment changed while you were editing it (version ${editFrom} → ${c.version}). Their text, then yours in the editor:`
                    : refreshing
                      ? 'This comment changed while you were editing it. Loading the newer text…'
                      : 'Confluence refused the save: the comment changed at the same moment. Save again, or discard your edit.'}
                </span>
              </div>
              {moved && <div className="wb-prose wb-cf cf-conflict-theirs" dangerouslySetInnerHTML={{ __html: sanitize(c.html) }} />}
              <div className="wb-row cf-conflict-actions">
                <Button size="small" variant="ghost" disabled={!!busy} onClick={discardMine}>
                  Discard mine
                </Button>
                <Button size="small" variant="danger" disabled={!moved || !!busy} onClick={() => void overwrite()} title="Replace their text with yours">
                  Overwrite with mine
                </Button>
              </div>
            </div>
          )}
          {c.editLossy && (
            <div className="wb-row wb-xs wb-warning" style={{ margin: '4px 0' }}>
              <AlertTriangle size={12} /> This comment has formatting markdown cannot keep (macros, page links…); saving simplifies it.
            </div>
          )}
          <Composer
            projectId={projectId}
            draftKey={editKey}
            placeholder="Edit the comment…"
            submitLabel="Save"
            autoFocus
            onCancel={() => setConflict(false)}
            onSubmit={(md) => save(md, editFrom)}
          />
        </>
      ) : (
        <div className="wb-prose wb-cf" dangerouslySetInnerHTML={{ __html: sanitize(c.html) }} />
      )}
      {c.replies.length > 0 && (
        <div className="cf-comment-replies">
          {c.replies.map((r) => (
            <CommentCard key={r.id} c={r} projectId={projectId} pageId={pageId} me={me} refreshing={refreshing} onChanged={onChanged} />
          ))}
        </div>
      )}
      {onReply &&
        !editing &&
        (replying ? (
          <Composer
            projectId={projectId}
            draftKey={replyKey}
            placeholder="Reply…"
            autoFocus
            submitLabel="Reply"
            onCancel={() => setReplying(false)}
            onSubmit={async (md) => {
              const ok = await onReply(md)
              if (ok) setReplying(false)
              return ok
            }}
          />
        ) : (
          <Button size="small" variant="ghost" icon={Reply} onClick={() => setReplying(true)} style={{ marginTop: 4 }}>
            Reply
          </Button>
        ))}
    </div>
  )
}

export function CommentsPane({
  projectId,
  page,
  activeRef,
  draft,
  onDraftDone,
  onJumpToMarker,
  onActivate,
  onClose,
}: {
  projectId: string | null
  page: Page
  activeRef: string | null
  /** A new inline comment on this selection is being written. */
  draft: Anchor | null
  onDraftDone: (markerRef: string | null) => void
  onJumpToMarker: (ref: string) => boolean
  onActivate: (ref: string | null) => void
  onClose: () => void
}) {
  const qc = useQueryClient()
  const q = useComments(projectId, page.id, true)
  const me = useAtlassianStatus((s) => s.status?.user?.accountId ?? null)
  const draftKeyNow = draft ? inlineDraftKey(projectId, page.id, draft) : undefined
  // Inline comments written earlier and not posted (the pane was closed, the page reloaded).
  const allDrafts = useCommentDrafts((s) => s.drafts)
  const pending = useMemo(() => pendingInline(allDrafts, projectId, page.id, draftKeyNow), [allDrafts, projectId, page.id, draftKeyNow])
  const [tab, setTab] = useState<'page' | 'inline'>(activeRef || draft || pending.length ? 'inline' : 'page')
  const [showResolved, setShowResolved] = useState(false)
  const markerSet = useMemo(() => new Set(page.inlineMarkerRefs), [page.inlineMarkerRefs])
  const readOnly = page.status !== 'current' || page.historical
  // Clicking a highlight in the page, or selecting text to comment on, shows the inline tab.
  useEffect(() => {
    if (activeRef || draft) setTab('inline')
  }, [activeRef, draft])
  const activeResolved = !!activeRef && q.data?.inline.some((c) => c.markerRef === activeRef && c.resolutionStatus === 'resolved')
  useEffect(() => {
    if (activeResolved) setShowResolved(true)
  }, [activeResolved])

  const refresh = () => {
    qc.invalidateQueries({ queryKey: qk.comments(projectId, page.id) })
    qc.invalidateQueries({ queryKey: qk.page(projectId, page.id) })
  }

  const add = async (markdown: string, parent?: Comment): Promise<boolean> => {
    try {
      await confluenceApi.addComment(projectId, page.id, { markdown, parentCommentId: parent?.id, parentKind: parent?.kind })
      toast('success', parent ? 'Reply posted' : 'Comment posted')
      qc.invalidateQueries({ queryKey: qk.comments(projectId, page.id) })
      return true
    } catch (e) {
      toastError(e, 'Could not post the comment')
      return false
    }
  }

  const addInline = async (anchor: Anchor, markdown: string): Promise<boolean> => {
    try {
      const out = await confluenceApi.addInlineComment(projectId, page.id, { markdown, ...anchor })
      toast('success', 'Inline comment added')
      refresh()
      onDraftDone(out.markerRef)
      return true
    } catch (e) {
      toastError(e, 'Could not add the inline comment')
      return false
    }
  }

  const footer = q.data?.footer ?? []
  const inlineAll = q.data?.inline ?? []
  const inline = inlineAll.filter((c) => showResolved || c.resolutionStatus !== 'resolved')
  const openInline = inlineAll.filter((c) => c.resolutionStatus !== 'resolved').length
  const resolvedCount = inlineAll.length - openInline

  return (
    <div className="cf-side">
      <Toolbar>
        <Tabs
          tabs={[
            { id: 'page', label: 'Page', badge: <span className="wb-subtle wb-xs">{footer.length}</span> },
            { id: 'inline', label: 'Inline', badge: <span className="wb-subtle wb-xs">{openInline}</span> },
          ]}
          value={tab}
          onChange={setTab}
        />
        <Spacer />
        {q.isFetching && !q.isLoading && <Spinner size={11} />}
        <IconButton size="small" icon={X} label="Close comments" onClick={onClose} />
      </Toolbar>
      {q.error && q.data && <StaleNotice what="the comments" error={q.error} onRetry={() => void q.refetch()} />}
      {q.isLoading ? (
        <Loading label="Loading comments…" />
      ) : q.error && !q.data ? (
        <ErrorBox error={q.error} onRetry={() => q.refetch()} />
      ) : tab === 'page' ? (
        <>
          <div className="cf-comments-list">
            {footer.length === 0 && <EmptyState icon={MessageSquare} title="No comments yet" />}
            {footer.map((c) => (
              <CommentCard
                key={c.id}
                c={c}
                projectId={projectId}
                pageId={page.id}
                me={me}
                refreshing={q.isFetching}
                onReply={readOnly ? undefined : (md) => add(md, c)}
                onChanged={refresh}
              />
            ))}
          </div>
          {!readOnly && <Composer projectId={projectId} draftKey={draftKey(projectId, page.id, 'footer')} placeholder="Add a comment…" onSubmit={(md) => add(md)} />}
        </>
      ) : (
        <div className="cf-comments-list">
          {draft && (
            <div className="cf-comment draft">
              <div className="cf-comment-head">
                <MessageSquarePlus size={13} />
                <span className="who">New inline comment</span>
              </div>
              <div className="cf-comment-quote pending">{draft.selection}</div>
              <Composer
                projectId={projectId}
                draftKey={inlineDraftKey(projectId, page.id, draft)}
                anchor={draft}
                placeholder="Comment on the selected text…"
                autoFocus
                onCancel={() => onDraftDone(null)}
                onSubmit={(md) => addInline(draft, md)}
              />
            </div>
          )}
          {!readOnly &&
            pending.map(({ key, anchor }) => (
              <div key={key} className="cf-comment draft">
                <div className="cf-comment-head">
                  <MessageSquarePlus size={13} />
                  <span className="who">Unsent inline comment</span>
                </div>
                <div className="cf-comment-quote pending" title="The text it is about">
                  {anchor.selection}
                </div>
                <Composer projectId={projectId} draftKey={key} anchor={anchor} placeholder="Comment on this text…" onCancel={() => undefined} onSubmit={(md) => addInline(anchor, md)} />
              </div>
            ))}
          <div className="wb-row" style={{ padding: '2px 2px 8px' }}>
            <Checkbox checked={showResolved} onChange={setShowResolved}>
              <span className="wb-small">Show resolved{resolvedCount ? ` (${resolvedCount})` : ''}</span>
            </Checkbox>
          </div>
          {inline.length === 0 && !draft && !pending.length && (
            <EmptyState icon={MessageSquare} title={showResolved ? 'No inline comments' : 'No open inline comments'}>
              {readOnly ? null : 'Select text in the page and choose Comment (Ctrl+Alt+C).'}
            </EmptyState>
          )}
          {inline.map((c) => (
            <CommentCard
              key={c.id}
              c={c}
              projectId={projectId}
              pageId={page.id}
              me={me}
              refreshing={q.isFetching}
              thread={!readOnly}
              active={!!activeRef && c.markerRef === activeRef}
              detached={!!c.markerRef && !markerSet.has(c.markerRef)}
              onQuote={() => {
                onActivate(c.markerRef)
                if (!c.markerRef || !onJumpToMarker(c.markerRef)) toast('info', 'This comment’s highlight is no longer in the page')
              }}
              onReply={readOnly ? undefined : (md) => add(md, c)}
              onChanged={refresh}
            />
          ))}
          {q.data?.truncated && <div className="wb-xs wb-subtle">Showing the first 250 comments.</div>}
        </div>
      )}
    </div>
  )
}
