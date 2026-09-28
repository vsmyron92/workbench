// Page operations: move (into / before / after another page), copy, move to the
// trash (typed confirmation) with an undo that restores it.

import { useState } from 'react'
import { useQueryClient, type QueryClient } from '@tanstack/react-query'
import { closePanel, confirmDialog, toast, toastError } from '@/shell/actions'
import { Button, Checkbox, ErrorBox, Field, Input, Modal, Tabs } from '@/ui'
import { confluenceApi, usePage, type SearchHit } from '../api'
import { openConfluencePage } from './actions'
import { PagePicker } from './PagePicker'

/** Refresh everything that shows where pages are (trees, lists, crumbs). */
export function invalidateTree(qc: QueryClient, pageId?: string) {
  qc.invalidateQueries({
    predicate: (q) => {
      const k = q.queryKey as unknown[]
      if (k[0] !== 'confluence') return false
      if (['children', 'roots', 'byIds', 'search', 'pick'].includes(String(k[1]))) return true
      return !!pageId && k[1] === 'page' && k[3] === pageId
    },
  })
}

export interface PageRef {
  id: string
  title: string
}

type Position = 'append' | 'before' | 'after'

export function MoveDialog({ projectId, page, onClose }: { projectId: string | null; page: PageRef; onClose: () => void }) {
  const qc = useQueryClient()
  const [position, setPosition] = useState<Position>('append')
  const [target, setTarget] = useState<SearchHit | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)

  const move = async (t = target) => {
    if (!t) return
    setBusy(true)
    setError(null)
    try {
      await confluenceApi.move(projectId, page.id, position, t.id)
      toast('success', position === 'append' ? `Moved “${page.title}” into “${t.title}”` : `Moved “${page.title}” ${position} “${t.title}”`)
      invalidateTree(qc, page.id)
      onClose()
    } catch (e) {
      setError(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      title={`Move “${page.title}”`}
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!target} onClick={() => void move()}>
            Move
          </Button>
        </>
      }
    >
      <Tabs
        tabs={[
          { id: 'append', label: 'Into a page' },
          { id: 'before', label: 'Before a page' },
          { id: 'after', label: 'After a page' },
        ]}
        value={position}
        onChange={setPosition}
      />
      <div className="wb-xs wb-subtle" style={{ margin: '6px 0' }}>
        {position === 'append'
          ? 'The page becomes the last child of the page you pick; its own child pages move with it.'
          : `The page becomes a sibling of the page you pick, ${position} it (not next to a top-level page: Confluence's tree hides those).`}
      </div>
      <PagePicker projectId={projectId} value={target} onChange={setTarget} exclude={[page.id]} autoFocus onEnter={(t) => void move(t)} />
      {error !== null && <ErrorBox error={error} />}
    </Modal>
  )
}

export function CopyDialog({ projectId, page, onClose }: { projectId: string | null; page: PageRef; onClose: () => void }) {
  const qc = useQueryClient()
  const [title, setTitle] = useState(`Copy of ${page.title}`)
  const [where, setWhere] = useState<'here' | 'under'>('here')
  const [parent, setParent] = useState<SearchHit | null>(null)
  const [attachments, setAttachments] = useState(true)
  const [labels, setLabels] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)
  // A top-level page (the space home, say) has no "next to it" in Confluence's tree:
  // its copy needs a parent. The tree and search do not always know the parent, so ask.
  const full = usePage(projectId, page.id)
  const topLevel = !!full.data && !full.data.parentId
  const place = topLevel ? 'under' : where
  const ready = !!title.trim() && (place === 'here' || !!parent)

  const copy = async () => {
    if (!ready) return
    setBusy(true)
    setError(null)
    try {
      const out = await confluenceApi.copy(projectId, page.id, {
        title: title.trim(),
        parentId: place === 'under' ? parent?.id : undefined,
        copyAttachments: attachments,
        copyLabels: labels,
      })
      toast('success', `Copied to “${out.title}”`)
      invalidateTree(qc)
      onClose()
      openConfluencePage(out.id, out.title)
    } catch (e) {
      setError(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      title={`Copy “${page.title}”`}
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!ready} onClick={() => void copy()}>
            Copy
          </Button>
        </>
      }
    >
      <Field label="Title of the copy" hint="Titles are unique within a space.">
        <Input autoFocus value={title} onChange={(e) => setTitle(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && place === 'here' && void copy()} />
      </Field>
      {topLevel ? (
        <div className="wb-xs wb-subtle" style={{ margin: '6px 0' }}>
          “{page.title}” is a top-level page, and Confluence's page tree hides top-level pages: choose the page to put the copy under.
        </div>
      ) : (
        <Tabs
          tabs={[
            { id: 'here', label: 'Next to the original' },
            { id: 'under', label: 'Under another page' },
          ]}
          value={where}
          onChange={setWhere}
        />
      )}
      {place === 'under' && (
        <div style={{ marginTop: 8 }}>
          <PagePicker projectId={projectId} value={parent} onChange={setParent} />
        </div>
      )}
      <div className="wb-row" style={{ gap: 16, marginTop: 10 }}>
        <Checkbox checked={attachments} onChange={setAttachments}>
          Include attachments
        </Checkbox>
        <Checkbox checked={labels} onChange={setLabels}>
          Include labels
        </Checkbox>
      </div>
      <div className="wb-xs wb-subtle" style={{ marginTop: 6 }}>
        Child pages, comments and restrictions are not copied.
      </div>
      {error !== null && <ErrorBox error={error} />}
    </Modal>
  )
}

/** Move a page to the trash after the user types its title; the toast offers Restore. */
export async function trashPage(qc: QueryClient, projectId: string | null, page: PageRef) {
  const ok = await confirmDialog({
    title: `Move “${page.title}” to the trash?`,
    message: 'The page leaves the page tree. Confluence keeps it in the space trash, where it can be restored until a space admin purges it.',
    confirmLabel: 'Move to trash',
    danger: true,
    typed: page.title,
  })
  if (!ok) return false
  try {
    await confluenceApi.trash(projectId, page.id)
  } catch (e) {
    toastError(e, `Could not delete “${page.title}”`)
    return false
  }
  closePanel(`confluence:${page.id}`)
  // The page is gone: forget what was loaded for it rather than asking again.
  qc.removeQueries({
    predicate: (q) => {
      const k = q.queryKey as unknown[]
      return k[0] === 'confluence' && ['page', 'comments', 'attachments', 'watch', 'versions'].includes(String(k[1])) && k[3] === page.id
    },
  })
  invalidateTree(qc)
  toast('success', `“${page.title}” moved to the trash`, {
    detail: 'Restore it from here, or later from the space trash in Confluence.',
    timeout: 15_000,
    action: {
      label: 'Restore',
      run: () =>
        void confluenceApi
          .restore(projectId, page.id)
          .then(() => {
            invalidateTree(qc, page.id)
            toast('success', `Restored “${page.title}”`, { action: { label: 'Open', run: () => openConfluencePage(page.id, page.title) } })
          })
          .catch((e) => toastError(e, 'Could not restore the page')),
    },
  })
  return true
}
