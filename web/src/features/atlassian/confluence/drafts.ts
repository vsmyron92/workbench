// Unsent comment text (a new page or inline comment, a reply, an edit) kept until it is
// posted or cancelled, so switching the comments pane's tab, closing it, or a refresh
// that fails loses nothing. Mirrored to this tab's sessionStorage (it survives a reload;
// page drafts, which are larger and longer-lived, use localStorage).

import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import type { Anchor } from './selection'

export interface CommentDraft {
  text: string
  /** Edits: the comment version the edit started from; the save sends it (409 when it moved). */
  version?: number
  /** New inline comments: the text they are about, so the draft can be posted after the pane was closed. */
  anchor?: Anchor
  at: number
}

export type DraftKind = 'footer' | 'inline' | 'reply' | 'edit'

/** `footer`: the page's new-comment box; `inline`: `id` is the anchor; `reply` / `edit`: `id` is the comment. */
export function draftKey(projectId: string | null, pageId: string, kind: DraftKind, id = ''): string {
  return [projectId ?? '', pageId, kind, id].join('|')
}

export const inlineDraftKey = (projectId: string | null, pageId: string, a: Anchor) =>
  draftKey(projectId, pageId, 'inline', `${a.matchIndex}/${a.matchCount}:${a.selection}`)

/** Unsent inline comments of a page (with their anchors), newest first, except `skip`. */
export function pendingInline(drafts: Record<string, CommentDraft>, projectId: string | null, pageId: string, skip?: string) {
  const prefix = draftKey(projectId, pageId, 'inline')
  return Object.entries(drafts)
    .filter(([k, d]) => k.startsWith(prefix) && k !== skip && !!d.anchor && !!d.text.trim())
    .sort((a, b) => b[1].at - a[1].at)
    .map(([key, d]) => ({ key, anchor: d.anchor as Anchor }))
}

const MAX_DRAFTS = 50
const MAX_AGE_MS = 7 * 24 * 3600_000

/** The newest drafts, at most a week old. */
export function pruneDrafts(drafts: Record<string, CommentDraft>, now = Date.now()): Record<string, CommentDraft> {
  const kept = Object.entries(drafts)
    .filter(([, d]) => now - d.at < MAX_AGE_MS)
    .sort((a, b) => b[1].at - a[1].at)
    .slice(0, MAX_DRAFTS)
  return Object.fromEntries(kept)
}

/** Drop `key`; the same object when there is nothing to drop. */
export function withoutDraft(drafts: Record<string, CommentDraft>, key: string): Record<string, CommentDraft> {
  if (!(key in drafts)) return drafts
  const next = { ...drafts }
  delete next[key]
  return next
}

interface DraftState {
  drafts: Record<string, CommentDraft>
  put: (key: string, patch: Partial<Omit<CommentDraft, 'at'>>) => void
  clear: (key: string) => void
}

export const useCommentDrafts = create<DraftState>()(
  persist(
    (set) => ({
      drafts: {},
      put: (key, patch) =>
        set((s) => {
          const prev: Omit<CommentDraft, 'at'> = s.drafts[key] ?? { text: '' }
          return { drafts: pruneDrafts({ ...s.drafts, [key]: { ...prev, ...patch, at: Date.now() } }) }
        }),
      clear: (key) => set((s) => ({ drafts: withoutDraft(s.drafts, key) })),
    }),
    // createJSONStorage tolerates a storage that throws (private windows): drafts then live in memory only.
    { name: 'wb.atlassian.commentDrafts.v1', storage: createJSONStorage(() => sessionStorage), partialize: (s) => ({ drafts: s.drafts }) },
  ),
)
