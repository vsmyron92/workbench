// A folder of images as a grid; click for the lightbox (arrow keys browse).
// Subfolders are browsable, so one step can hold several rounds of images.

import { useEffect, useState } from 'react'
import { ChevronRight, Folder, Images } from 'lucide-react'
import { Button, EmptyState, ErrorBox, Loading, Toolbar } from '@/ui'
import { useCardFilePages, type WorkspaceCard } from '../api'
import { basename, fileUrl, isImageName, isVideoName } from '../logic'
import { Lightbox } from './Lightbox'

export function GalleryView({ card, path }: { card: WorkspaceCard; path: string }) {
  const [sub, setSub] = useState(path)
  const [open, setOpen] = useState<number | null>(null)
  useEffect(() => setSub(path), [path])
  const { data, error, isLoading, refetch, hasNextPage, fetchNextPage, isFetchingNextPage } = useCardFilePages(card.scope, card.id, sub)
  // Large folders come a page (2000 entries) at a time, in name order.
  const entries = data?.pages.flatMap((p) => p.entries) ?? []
  const total = data?.pages[0]?.total ?? entries.length
  const dirs = entries.filter((e) => e.dir)
  const images = entries.filter((e) => !e.dir && isImageName(e.name))
  const videos = entries.filter((e) => !e.dir && isVideoName(e.name))
  const items = images.map((e) => ({ url: fileUrl(card.base, e.path, e.mtime), name: e.name }))
  const crumbs = sub.startsWith(path) ? sub.slice(path.length).split('/').filter(Boolean) : []

  return (
    <div className="wb-fill">
      <Toolbar>
        <button className="ws-crumb" onClick={() => setSub(path)} disabled={sub === path}>
          <Images size={13} />
          {basename(path) || 'Folder'}
        </button>
        {crumbs.map((c, i) => (
          <span key={i} className="wb-row" style={{ gap: 2 }}>
            <ChevronRight size={12} className="wb-subtle" />
            <button className="ws-crumb" onClick={() => setSub([path, ...crumbs.slice(0, i + 1)].join('/'))} disabled={i === crumbs.length - 1}>
              {c}
            </button>
          </span>
        ))}
        <span className="spacer" />
        <span className="wb-small wb-muted">
          {images.length} image{images.length === 1 ? '' : 's'}
          {videos.length ? ` · ${videos.length} video${videos.length === 1 ? '' : 's'}` : ''}
          {hasNextPage ? ` · ${entries.length.toLocaleString()} of ${total.toLocaleString()} entries shown` : ''}
        </span>
      </Toolbar>
      <div className="wb-scroll">
        {error ? (
          <ErrorBox error={error} onRetry={() => void refetch()} />
        ) : isLoading ? (
          <Loading />
        ) : !entries.length ? (
          <EmptyState icon={Images} title="This folder is empty" />
        ) : (
          <div className="ws-gallery">
            {dirs.map((d) => (
              <button key={d.path} className="ws-gallery-item folder" onClick={() => setSub(d.path)} title={d.name}>
                <span className="ws-gallery-img">
                  <Folder size={32} />
                </span>
                <span className="ws-gallery-name wb-ellipsis">{d.name}</span>
              </button>
            ))}
            {images.map((e, i) => (
              <button key={e.path} className="ws-gallery-item" onClick={() => setOpen(i)} title={e.name}>
                <span className="ws-gallery-img">
                  <img src={items[i].url} alt={e.name} loading="lazy" decoding="async" draggable={false} />
                </span>
                <span className="ws-gallery-name wb-ellipsis">{e.name}</span>
              </button>
            ))}
            {videos.map((e) => (
              <div key={e.path} className="ws-gallery-item video" title={e.name}>
                <span className="ws-gallery-img">
                  <video src={fileUrl(card.base, e.path, e.mtime)} controls preload="metadata" />
                </span>
                <span className="ws-gallery-name wb-ellipsis">{e.name}</span>
              </div>
            ))}
            {hasNextPage && (
              <div className="ws-gallery-more">
                <Button size="small" loading={isFetchingNextPage} onClick={() => void fetchNextPage()}>
                  Show more ({(total - entries.length).toLocaleString()} left)
                </Button>
              </div>
            )}
          </div>
        )}
      </div>
      {open !== null && items[open] && <Lightbox items={items} index={open} onIndex={setOpen} onClose={() => setOpen(null)} />}
    </div>
  )
}
