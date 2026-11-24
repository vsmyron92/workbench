// The small face: an avatar's head on the app's stage, with the glow of its state, the
// unseen dot and a count. It stands for an avatar where there is no room for the figure:
// in the agents window's top bar, in the phone's header, in the status bar, and as the
// "+n" chip behind the fourth figure.
//
// It is drawn in the look's rest pose, without an engine: a face of 28 px is too small to
// read motion from. Pure views, like the figure: what is shown comes in as props.

import { memo, useMemo, type Ref } from 'react'
import { FACE_PX, StillLook, drawnLook, keepFocus, ringClass, sizeStyle, type Ring } from './Figure'

/** What is kept round a face's stage, in CSS px: nothing. No ring is drawn round a face (its state is a glow on its own outline). */
const FACE_RING_PX = 0

/** The stage of a face that is `size` px over all: the face is as large as the place it is given. */
function stageOf(size: number): number {
  return Math.max((Number.isFinite(size) ? Math.round(size) : FACE_PX.bar) - FACE_RING_PX, 8)
}

/** A count on a face is at most two digits wide. */
function countText(count: number | undefined): string | null {
  if (typeof count !== 'number' || !Number.isFinite(count) || count < 1) return null
  const n = Math.floor(count)
  return n > 99 ? '99+' : String(n)
}

export interface FaceMarkProps {
  /** The record's `look` as the server sent it, or null for the default look. */
  look: unknown
  /** The terminal id: the hue of the default look. */
  seed: string
  /** What the artwork's ids are printed with: unique on the page, and not the prefix of the same avatar's figure. */
  idPrefix: string
  /** The whole face in CSS px (`FACE_PX`): its stage fills it. */
  size?: number
  /** `ringFor(state, tone)`. */
  ring: Ring
  /** Its newest agent line was not seen: an accent dot. */
  unseen?: boolean
  /** A number on the face. Nothing is shown for 0. */
  count?: number
  /** `attention`: the count is of things that wait for the user (permission requests); else it only counts. */
  countTone?: 'attention' | 'plain'
}

/** The face itself, for a place that is a control already (a status-bar widget, a menu row). It takes no pointer. */
export const FaceMark = memo(function FaceMark({ look, seed, idPrefix, size = FACE_PX.bar, ring, unseen, count, countTone = 'plain' }: FaceMarkProps) {
  const drawn = useMemo(() => drawnLook(look, seed), [look, seed])
  const n = countText(count)
  const stage = stageOf(size)
  return (
    <span className={['wb-av-facemark', ringClass(ring), unseen && 'unseen'].filter(Boolean).join(' ')} style={sizeStyle(stage)}>
      <span className="wb-av-ring">
        <StillLook look={drawn.look} idPrefix={idPrefix} size={stage} face drawn={drawn.own ? 'own' : 'default'} />
      </span>
      {unseen && <span className="wb-av-dot" />}
      {n !== null && <span className={countTone === 'attention' ? 'wb-av-count attention' : 'wb-av-count'}>{n}</span>}
    </span>
  )
})

export interface FaceProps extends FaceMarkProps {
  /** The accessible name and the tooltip: who it is and what it does now. */
  label: string
  /** Its bubble is open (the compact panel under the top bar). */
  open?: boolean
  onPress?: () => void
  buttonRef?: Ref<HTMLButtonElement>
}

/** The face as a button: the compact form of an avatar. */
export const Face = memo(function Face({ label, open, onPress, buttonRef, ...mark }: FaceProps) {
  return (
    <button ref={buttonRef} type="button" className="wb-av-face" aria-label={label} title={label} aria-expanded={open === undefined ? undefined : open} onPointerDown={keepFocus} onMouseDown={keepFocus} onClick={onPress}>
      <FaceMark {...mark} />
    </button>
  )
})

export interface MoreFacesProps {
  /** How many avatars are behind the chip. */
  count: number
  /** The whole chip in CSS px, like a face's: as large as what stands beside it (beside figures, a resting figure's stage and `FIGURE_RING_PX`, which is 0). */
  size?: number
  /** The most urgent ring among them. */
  ring: Ring
  /** One of them has a line that was not seen. */
  unseen?: boolean
  label: string
  onPress?: () => void
  buttonRef?: Ref<HTMLButtonElement>
}

/** The "+n" chip: the avatars that do not fit beside the first four. It opens a menu. */
export function MoreFaces({ count, size = FACE_PX.bar, ring, unseen, label, onPress, buttonRef }: MoreFacesProps) {
  const n = Math.min(Math.max(Number.isFinite(count) ? Math.floor(count) : 0, 0), 99)
  const stage = stageOf(size)
  return (
    <button ref={buttonRef} type="button" className="wb-av-face" aria-label={label} title={label} aria-haspopup="menu" onPointerDown={keepFocus} onMouseDown={keepFocus} onClick={onPress}>
      <span className={['wb-av-facemark', ringClass(ring), unseen && 'unseen'].filter(Boolean).join(' ')} style={sizeStyle(stage)}>
        <span className="wb-av-ring">
          <span className="wb-av-stage wb-av-more" style={sizeStyle(stage)}>
            {`+${n}`}
          </span>
        </span>
        {unseen && <span className="wb-av-dot" />}
      </span>
    </button>
  )
}
