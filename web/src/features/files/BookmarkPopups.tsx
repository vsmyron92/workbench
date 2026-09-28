// Shift+F11 Bookmarks (typing a mnemonic jumps to it; Delete removes one) and the
// Ctrl+F11 mnemonic chooser.

import { useEffect, useMemo, useState } from 'react'
import { Command as Cmdk } from 'cmdk'
import { create } from 'zustand'
import { useUi } from '@/state/store'
import { Kbd, Modal } from '@/ui'
import { MNEMONICS, sortBookmarks, useBookmarks, type Bookmark } from './bookmarks'
import { bufferKey, getModel } from './buffers'
import { FileIcon } from './icons'
import { openFile } from './openers'
import { basename, dirname } from './paths'

export const useBookmarkPopup = create<{
  list: boolean
  chooser: { projectId: string | null; path: string; line: number } | null
  showList: () => void
  choose: (at: { projectId: string | null; path: string; line: number }) => void
  hide: () => void
}>()((set) => ({
  list: false,
  chooser: null,
  showList: () => set({ list: true, chooser: null }),
  choose: (chooser) => set({ chooser, list: false }),
  hide: () => set({ list: false, chooser: null }),
}))

export function BookmarkPopupHost() {
  const { list, chooser } = useBookmarkPopup()
  if (chooser) return <MnemonicChooser at={chooser} />
  return list ? <BookmarkList /> : null
}

function lineText(b: Bookmark): string {
  const m = getModel(bufferKey(b.projectId, b.path))
  if (!m || m.isDisposed() || b.line > m.getLineCount()) return ''
  return m.getLineContent(b.line).trim()
}

function open(b: Bookmark) {
  useBookmarkPopup.getState().hide()
  openFile({ projectId: b.projectId, path: b.path, line: b.line })
}

function BookmarkList() {
  const hide = useBookmarkPopup((s) => s.hide)
  const projectId = useUi((s) => s.projectId)
  const all = useBookmarks((s) => s.list)
  const [q, setQ] = useState('')
  const [selected, setSelected] = useState('')
  // This project's first; others after, marked with their project.
  const items = useMemo(() => {
    const sorted = sortBookmarks(all)
    return [...sorted.filter((b) => b.projectId === projectId), ...sorted.filter((b) => b.projectId !== projectId)].map((b) => ({ b, text: lineText(b) }))
  }, [all, projectId])
  const shown = useMemo(() => {
    const t = q.trim().toLowerCase()
    return t ? items.filter(({ b, text }) => b.path.toLowerCase().includes(t) || text.toLowerCase().includes(t)) : items
  }, [items, q])
  const value = shown.some((x) => x.b.id === selected) ? selected : (shown[0]?.b.id ?? '')

  return (
    <Cmdk.Dialog
      open
      onOpenChange={(o) => !o && hide()}
      label="Bookmarks"
      className="wb-palette wb-quickopen wb-bookmarks"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
      loop
      value={value}
      onValueChange={setSelected}
    >
      <div className="wb-quickopen-heading wb-recent-heading">
        Bookmarks
        <span className="wb-grow" />
        <span className="wb-subtle">type a mnemonic to jump</span>
      </div>
      <Cmdk.Input
        value={q}
        onValueChange={(v) => {
          // Like CLion: a mnemonic typed into the empty list jumps straight to it.
          if (!q && v.length === 1) {
            const hit = items.find(({ b }) => b.mnemonic === v.toUpperCase())
            if (hit) {
              open(hit.b)
              return
            }
          }
          setQ(v)
        }}
        placeholder="Filter, or type a mnemonic"
        autoFocus
        onKeyDown={(e) => {
          if (e.key === 'Delete' && value) {
            e.preventDefault()
            useBookmarks.getState().remove(value)
          }
        }}
      />
      <Cmdk.List>
        {!shown.length && <Cmdk.Empty>{items.length ? 'Nothing matches.' : 'No bookmarks: F11 bookmarks the caret line, Ctrl+F11 with a mnemonic.'}</Cmdk.Empty>}
        {shown.map(({ b, text }) => (
          <Cmdk.Item key={b.id} value={b.id} onSelect={() => open(b)}>
            <span className={b.mnemonic ? 'wb-bm-badge' : 'wb-bm-badge plain'}>{b.mnemonic ?? ''}</span>
            <FileIcon path={b.path} />
            <span className="wb-quickopen-name">
              {basename(b.path)}:{b.line}
            </span>
            <span className="wb-bm-text wb-ellipsis">{text}</span>
            <span className="wb-quickopen-dir wb-ellipsis">
              {b.projectId !== projectId ? `${b.projectId ?? ''} · ` : ''}
              {dirname(b.path) === '/' ? '' : dirname(b.path)}
            </span>
          </Cmdk.Item>
        ))}
      </Cmdk.List>
      <div className="wb-quickopen-footer">
        <span className="wb-grow" />
        <Kbd>Enter</Kbd> open <Kbd>Del</Kbd> remove
      </div>
    </Cmdk.Dialog>
  )
}

function MnemonicChooser({ at }: { at: { projectId: string | null; path: string; line: number } }) {
  const hide = useBookmarkPopup((s) => s.hide)
  const list = useBookmarks((s) => s.list)
  const used = useMemo(() => new Map(list.filter((b) => b.mnemonic).map((b) => [b.mnemonic!, b])), [list])
  const here = list.find((b) => b.projectId === at.projectId && b.path === at.path && b.line === at.line)
  const pick = (m: string) => {
    useBookmarks.getState().toggle(at.projectId, at.path, at.line, m)
    hide()
  }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || e.key.length !== 1) return
      const m = e.key.toUpperCase()
      if (!MNEMONICS.includes(m)) return
      e.preventDefault()
      e.stopPropagation()
      pick(m)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  return (
    <Modal title={`Bookmark ${basename(at.path)}:${at.line} with a mnemonic`} onClose={hide}>
      <div className="wb-bm-grid" role="group" aria-label="Mnemonic">
        {MNEMONICS.map((m) => {
          const b = used.get(m)
          const mine = b && here && b.id === here.id
          return (
            <button
              key={m}
              type="button"
              className={mine ? 'mine' : b ? 'used' : ''}
              title={b ? `${mine ? 'This line' : `${basename(b.path)}:${b.line}`}${mine ? '' : ' (moves here)'}` : undefined}
              onClick={() => pick(m)}
            >
              {m}
            </button>
          )
        })}
      </div>
      <p className="wb-small wb-muted">
        Press a digit or a letter. Taken ones move here; the line's own one removes the bookmark.
      </p>
    </Modal>
  )
}
