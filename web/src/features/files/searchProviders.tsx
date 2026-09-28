// Search Everywhere tabs of the files slice: Files (Go to File's ranking, recent
// files on an empty query, `name:12` opens at a line) and Text (Find in Files).

import { TextSearch } from 'lucide-react'
import type { SearchItem, SearchProvider } from '@/shell/types'
import { filesApi } from './api'
import { FileIcon } from './icons'
import { openFile } from './openers'
import { basename, dirname, parseGoto } from './paths'
import { rankItems } from './quickOpenModel'
import { recentFiles } from './store'

export const filesSearch: SearchProvider = {
  id: 'files',
  title: 'Files',
  order: 10,
  minQuery: 0,
  inAll: 6,
  when: (c) => !!c.projectId,
  search: async (input, ctx, signal) => {
    const pid = ctx.projectId!
    const goto = parseGoto(input)
    const recent = recentFiles(pid)
    const found = goto.query ? await filesApi.find(pid, goto.query, 60, signal) : null
    return rankItems(goto.query, found?.results ?? [], recent).map<SearchItem>((it) => {
      const name = basename(it.path)
      const offset = it.path.length - name.length
      return {
        key: it.path,
        title: name,
        highlight: it.positions.filter((p) => p >= offset).map((p) => p - offset),
        detail: dirname(it.path),
        icon: <FileIcon path={it.path} />,
        hint: it.recent ? 'recent' : undefined,
        run: ({ side }) => openFile({ projectId: pid, path: it.path, line: goto.line, column: goto.column, side }),
      }
    })
  },
}

export const textSearch: SearchProvider = {
  id: 'text',
  title: 'Text',
  order: 40,
  minQuery: 3,
  inAll: 4,
  when: (c) => !!c.projectId,
  search: async (q, ctx, signal) => {
    const pid = ctx.projectId!
    const r = await filesApi.search(pid, { q, regex: false, case: false, word: false, glob: '' }, 60, signal)
    return r.matches.map<SearchItem>((h) => {
      const lead = h.preview.length - h.preview.trimStart().length
      const start = h.column - 1 - h.previewOffset - lead
      const len = h.endColumn - h.column
      return {
        key: `${h.path}:${h.line}:${h.column}`,
        title: h.preview.trim(),
        highlight: start >= 0 ? Array.from({ length: len }, (_, i) => start + i) : undefined,
        detail: `${h.path}:${h.line}`,
        icon: <TextSearch size={15} />,
        run: ({ side }) => openFile({ projectId: pid, path: h.path, line: h.line, column: h.column, endColumn: h.endColumn, side }),
      }
    })
  },
}
