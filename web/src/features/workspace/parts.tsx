// Small building blocks shared by the home grid, the card panel, the tool window
// and the phone view.

import { useState, type ComponentType } from 'react'
import {
  BarChart3,
  BookOpen,
  Box,
  Braces,
  FileCode2,
  FileText,
  File as FileIcon,
  Film,
  FolderKanban,
  Gamepad2,
  Image,
  Images,
  LayoutGrid,
  Microscope,
  Music,
  Palette,
  PenTool,
  Rocket,
  Wrench,
} from 'lucide-react'
import { Badge } from '@/ui'
import type { ViewerKind, WorkspaceCard } from './api'
import { thumbUrl } from './logic'

type Icon = ComponentType<{ size?: number; className?: string }>

const CATEGORY_ICONS: [RegExp, Icon][] = [
  [/research|study|investig/, Microscope],
  [/analytic|metric|data|stat|report/, BarChart3],
  [/image|art|concept|visual/, Palette],
  [/design|ui|ux|mock/, PenTool],
  [/3d|model|mesh/, Box],
  [/game|play/, Gamepad2],
  [/dev|code|tech|engineer/, Wrench],
  [/doc|guide|note|spec|plan/, BookOpen],
  [/release|deploy|launch/, Rocket],
  [/project/, FolderKanban],
]

export function categoryIcon(category: string): Icon {
  const c = category.toLowerCase()
  return CATEGORY_ICONS.find(([re]) => re.test(c))?.[1] ?? LayoutGrid
}

export function CategoryIcon({ category, size = 14 }: { category: string; size?: number }) {
  const I = categoryIcon(category)
  return <I size={size} />
}

const KIND_ICONS: Record<ViewerKind, Icon> = {
  html: FileCode2,
  markdown: BookOpen,
  image: Image,
  gallery: Images,
  compare3d: Box,
  pdf: FileText,
  video: Film,
  audio: Music,
  text: Braces,
  file: FileIcon,
}

export function KindIcon({ kind, size = 13 }: { kind: ViewerKind; size?: number }) {
  const I = KIND_ICONS[kind] ?? FileIcon
  return <I size={size} />
}

export function StatusBadge({ card }: { card: WorkspaceCard }) {
  if (card.status === 'archived') return <Badge>Archived</Badge>
  if (card.archived) return <Badge title="Not touched for 7 days">Archived</Badge>
  if (card.status === 'done') return <Badge tone="accent">Done</Badge>
  return <Badge tone="success">Active</Badge>
}

/** The card's picture, or its category icon on a quiet tile. */
export function CardThumb({ card, size, className }: { card: WorkspaceCard; size?: number; className?: string }) {
  const url = thumbUrl(card)
  const [failed, setFailed] = useState<string | null>(null)
  const cls = ['ws-thumb', className].filter(Boolean).join(' ')
  const style = size ? { width: size, height: size } : undefined
  if (url && failed !== url) {
    return (
      <span className={cls} style={style}>
        <img src={url} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(url)} />
      </span>
    )
  }
  return (
    <span className={`${cls} empty`} style={style}>
      <CategoryIcon category={card.category} size={size ? Math.max(12, Math.round(size * 0.5)) : 30} />
    </span>
  )
}
