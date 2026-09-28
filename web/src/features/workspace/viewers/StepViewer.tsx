// Picks the viewer for a step (or a file previewed from the card folder).

import { lazy, Suspense, useMemo } from 'react'
import { Loading } from '@/ui'
import type { ViewerKind, WorkspaceCard } from '../api'
import { basename, fileUrl, singleModelManifest } from '../logic'
import { FileView, HtmlView, ImageView, MediaView, MissingView, PdfView, TextView } from './basic'
import { GalleryView } from './GalleryView'
import { MarkdownView } from './MarkdownView'

const Compare3D = lazy(() => import('../compare3d/Compare3D'))

export interface ViewTarget {
  path: string
  name: string
  kind: ViewerKind
  exists: boolean
  size?: number
  mtime?: number
}

export function StepViewer({
  card,
  target,
  onOpenPath,
  active,
}: {
  card: WorkspaceCard
  target: ViewTarget
  onOpenPath?: (path: string) => boolean
  active?: boolean
}) {
  const single = target.kind === 'compare3d' && /\.(glb|gltf)$/i.test(target.path)
  const manifest = useMemo(() => (single ? singleModelManifest(target.path, target.name) : undefined), [single, target.path, target.name])
  if (!target.exists) return <MissingView path={target.path} />
  const url = fileUrl(card.base, target.path, target.kind === 'html' || target.kind === 'image' ? target.mtime : undefined)
  const name = basename(target.path)
  switch (target.kind) {
    case 'html':
      return <HtmlView key={target.path} url={url} title={target.name} label={target.path} />
    case 'markdown':
      return <MarkdownView card={card} path={target.path} onOpenPath={onOpenPath} active={active} />
    case 'image':
      return <ImageView url={url} name={name} />
    case 'gallery':
      return <GalleryView card={card} path={target.path} />
    case 'video':
    case 'audio':
      return <MediaView url={url} name={name} kind={target.kind} />
    case 'pdf':
      return <PdfView url={url} name={name} />
    case 'text':
      return <TextView card={card} path={target.path} name={target.name} url={url} />
    case 'compare3d':
      return (
        <Suspense fallback={<Loading label="Loading the 3D viewer…" />}>
          <Compare3D key={target.path} manifestUrl={url} manifest={manifest} />
        </Suspense>
      )
    default:
      return <FileView url={url} name={name} size={target.size} />
  }
}
