// The Workspace trash (inside the home panel): deleted cards with their files,
// restorable until deleted for good. Restore puts a card back (under a free id or
// folder if a new card took its old one); Delete Permanently and Empty Trash ask
// for a typed confirmation.

import { useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ArchiveRestore, FolderOpen, Search, Trash2, TriangleAlert } from 'lucide-react'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Button, EmptyState, ErrorBox, IconButton, Loading, TimeAgo, formatBytes } from '@/ui'
import { type TrashItem, useTrash, wk, wsApi } from './api'
import { restoreFromTrash } from './actions'
import { ALL, categoryLabel } from './logic'
import { CategoryIcon } from './parts'

/** Items whose title, description, category or folder match `query`. */
export function filterTrash(items: TrashItem[], query: string): TrashItem[] {
  const q = query.trim().toLowerCase()
  if (!q) return items
  return items.filter((i) => [i.title, i.description, i.category, i.folder ?? '', i.scopeName].some((s) => s.toLowerCase().includes(q)))
}

export function TrashView({ scope, query }: { scope: string; query: string }) {
  const qc = useQueryClient()
  const { data, error, isLoading, refetch } = useTrash(scope)
  const [busy, setBusy] = useState<string | null>(null)
  const items = useMemo(() => filterTrash(data?.items ?? [], query), [data, query])
  const all = data?.items ?? []
  const bytes = all.reduce((n, i) => n + i.bytes, 0)

  const restore = async (i: TrashItem) => {
    setBusy(i.id)
    try {
      await restoreFromTrash(qc, i.scope, i.id, i.title)
    } finally {
      setBusy(null)
    }
  }

  const purge = async (i: TrashItem) => {
    const ok = await confirmDialog({
      title: `Delete "${i.title}" permanently?`,
      message: `${i.moved ? `Its ${i.files} file${i.files === 1 ? '' : 's'} (${formatBytes(i.bytes)}) are deleted` : 'Its entry is deleted'} for good. This cannot be undone.`,
      confirmLabel: 'Delete Permanently',
      danger: true,
      typed: 'delete',
    })
    if (!ok) return
    setBusy(i.id)
    try {
      await wsApi.purge(i.scope, i.id)
      void qc.invalidateQueries({ queryKey: wk.trash(scope) })
      toast('success', `Deleted "${i.title}" permanently`)
    } catch (e) {
      toastError(e, `Could not delete "${i.title}"`)
    } finally {
      setBusy(null)
    }
  }

  const empty = async () => {
    const ok = await confirmDialog({
      title: scope === ALL ? 'Empty every Workspace trash?' : 'Empty the Workspace trash?',
      message: `${all.length} card${all.length === 1 ? '' : 's'} and ${formatBytes(bytes)} of files are deleted for good. This cannot be undone.`,
      confirmLabel: 'Empty Trash',
      danger: true,
      typed: 'empty trash',
    })
    if (!ok) return
    setBusy('*')
    try {
      const r = await wsApi.emptyTrash(scope)
      void qc.invalidateQueries({ queryKey: ['workspace', 'trash'] })
      toast('success', `Emptied the trash (${r.removed} card${r.removed === 1 ? '' : 's'})`)
    } catch (e) {
      toastError(e, 'Could not empty the trash')
    } finally {
      setBusy(null)
    }
  }

  if (error) return <ErrorBox error={error} onRetry={() => void refetch()} />
  if (isLoading) return <Loading label="Loading the trash…" />
  return (
    <div className="ws-trash">
      <div className="ws-trash-head">
        <div className="ws-home-sub">
          <b>{all.length}</b> deleted card{all.length === 1 ? '' : 's'} · <b>{formatBytes(bytes)}</b>
          {query.trim() && (
            <span>
              {' '}
              · {items.length} match{items.length === 1 ? '' : 'es'} for “{query.trim()}”
            </span>
          )}
        </div>
        <span className="spacer" />
        <Button size="small" variant="danger" icon={Trash2} disabled={!all.length || busy !== null} loading={busy === '*'} onClick={() => void empty()}>
          Empty Trash
        </Button>
      </div>
      {items.length === 0 ? (
        query.trim() ? (
          <EmptyState icon={Search} title="Nothing in the trash matches" />
        ) : (
          <EmptyState icon={Trash2} title="The trash is empty">
            Deleted cards land here with their files, and can be restored until you empty the trash.
          </EmptyState>
        )
      ) : (
        <div className="ws-trash-list" role="list">
          {items.map((i) => (
            <div key={`${i.scope}/${i.id}`} className="ws-trash-row" role="listitem">
              <span className="ws-trash-icon">
                <CategoryIcon category={i.category} size={16} />
              </span>
              <div className="ws-trash-main">
                <div className="ws-trash-title wb-ellipsis" title={i.title}>
                  {i.title}
                </div>
                {i.description && <div className="ws-trash-desc wb-ellipsis">{i.description}</div>}
                <div className="ws-trash-meta">
                  {scope === ALL && <span className="ws-chip">{i.scopeName}</span>}
                  {i.category && <span className="ws-chip">{categoryLabel(i.category)}</span>}
                  <span>
                    deleted <TimeAgo time={i.deletedAt} />
                  </span>
                  <span>·</span>
                  <span title={i.folder ?? undefined}>
                    {i.moved ? (
                      i.files === 0 && !i.filesCapped ? (
                        'no files'
                      ) : (
                        <>
                          {i.files}
                          {i.filesCapped ? '+' : ''} file{i.files === 1 ? '' : 's'} · {formatBytes(i.bytes)}
                        </>
                      )
                    ) : (
                      <>
                        <FolderOpen size={11} /> folder kept (shared)
                      </>
                    )}
                  </span>
                </div>
                {i.problem && (
                  <div className="ws-trash-problem">
                    <TriangleAlert size={12} /> {i.problem}
                  </div>
                )}
              </div>
              <div className="ws-trash-actions">
                <Button size="small" icon={ArchiveRestore} disabled={!i.restorable || busy !== null} loading={busy === i.id} onClick={() => void restore(i)}>
                  Restore
                </Button>
                <IconButton icon={Trash2} size="small" label="Delete permanently…" disabled={busy !== null} onClick={() => void purge(i)} />
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
