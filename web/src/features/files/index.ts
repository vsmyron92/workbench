// Feature slice: files. Owned by the files slice — see docs/ARCHITECTURE.md.
// Project tree, editor, Markdown preview, find in files and Go to File.

import { lazy } from 'react'
import { ArrowLeft, ArrowRight, BookMarked, BookOpen, Columns2, FileDiff, Clock, Crosshair, FileCode, FileDown, FilePlus, FileSearch, FolderTree, History, ListRestart, ListTodo, MapPin, NotebookPen, Tag, TextSearch } from 'lucide-react'
import { showToolWindow, toast } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { exportAsHtml } from './export/ExportDialog'
import { newEntry } from './fileActions'
import { navigateBack, navigateForward } from './navigation'
import { useRecentPopup } from './RecentPopups'
import { useBookmarkPopup } from './BookmarkPopups'
import { FilesProvider } from './FilesProvider'
import { FilesToolWindow } from './FilesToolWindow'
import { putLabel, showLocalHistory, showRecentChanges } from './history/open'
import { MobileFiles } from './MobileFiles'
import { compareWithClipboard, compareWithPicked, revealInTree, showSearch } from './openers'
import { dirname, isMarkdown } from './paths'
import { chooseScratchKind } from './scratches'
import { useFilesView } from './scratchStore'
import { SearchPanel, SearchToolWindow } from './SearchView'
import { filesSearch, textSearch } from './searchProviders'
import { useActiveEditor, useQuickOpen } from './store'
import './files.css'

// Monaco-backed panels load with their own chunk.
const EditorPanel = lazy(() => import('./EditorPanel').then((m) => ({ default: m.EditorPanel })))
const MarkdownPanel = lazy(() => import('./MarkdownPanel').then((m) => ({ default: m.MarkdownPanel })))
const TodoToolWindow = lazy(() => import('./TodoToolWindow').then((m) => ({ default: m.TodoToolWindow })))
const ComparePanel = lazy(() => import('./ComparePanel').then((m) => ({ default: m.ComparePanel })))
const AgentChangesPanel = lazy(() => import('./history/AgentChangesPanel').then((m) => ({ default: m.AgentChangesPanel })))
const LocalHistoryPanel = lazy(() => import('./history/LocalHistoryPanel').then((m) => ({ default: m.LocalHistoryPanel })))

const feature: FeatureModule = {
  id: 'files',
  panels: {
    editor: { component: EditorPanel, keepAlive: true, icon: FileCode },
    markdown: { component: MarkdownPanel, icon: BookOpen },
    search: { component: SearchPanel, icon: TextSearch },
    localHistory: { component: LocalHistoryPanel, icon: Clock },
    agentChanges: { component: AgentChangesPanel, icon: FileDiff },
    compare: { component: ComparePanel, icon: Columns2 },
  },
  searchProviders: [filesSearch, textSearch],
  toolWindows: [
    { id: 'files', title: 'Files', icon: FolderTree, side: 'left', order: 10, component: FilesToolWindow },
    { id: 'search', title: 'Find', icon: TextSearch, side: 'left', order: 40, component: SearchToolWindow },
    { id: 'todo', title: 'TODO', icon: ListTodo, side: 'bottom', order: 16, component: TodoToolWindow },
  ],
  commands: (ctx) => [
    {
      id: 'files.gotoFile',
      title: 'Go to File…',
      group: 'Start',
      shortcut: 'mod+p',
      keywords: ['open', 'quick open', 'file'],
      icon: FileSearch,
      when: (c) => !!c.projectId,
      run: () => useQuickOpen.getState().show(),
    },
    { id: 'files.showFiles', title: 'Show Files', group: 'Tool windows', shortcut: 'alt+1', keywords: ['project', 'tree'], icon: FolderTree, run: () => showToolWindow('files', 'left') },
    { id: 'files.showFind', title: 'Show Find', group: 'Tool windows', shortcut: 'alt+3', keywords: ['search results'], icon: TextSearch, run: () => showToolWindow('search', 'left') },
    {
      id: 'files.recentFiles',
      title: 'Recent Files',
      group: 'Start',
      shortcut: 'mod+e',
      keywords: ['switcher', 'recently opened', 'changed files', 'recent'],
      icon: History,
      when: (c) => !!c.projectId,
      run: () => useRecentPopup.getState().show('files'),
    },
    {
      id: 'files.recentLocations',
      title: 'Recent Locations',
      group: 'Navigate',
      shortcut: 'mod+shift+e',
      keywords: ['places', 'caret', 'history', 'where was I'],
      icon: MapPin,
      when: (c) => !!c.projectId,
      run: () => useRecentPopup.getState().show('locations'),
    },
    {
      id: 'files.compareClipboard',
      title: 'Compare with Clipboard',
      group: 'Files',
      keywords: ['diff', 'paste'],
      icon: Columns2,
      when: () => !!useActiveEditor.getState().current?.projectId,
      run: () => {
        const a = useActiveEditor.getState().current
        if (a) void compareWithClipboard(a.projectId, a.path)
      },
    },
    {
      id: 'files.compareWith',
      title: 'Compare With…',
      group: 'Files',
      keywords: ['diff', 'compare files'],
      icon: Columns2,
      when: () => !!useActiveEditor.getState().current?.projectId,
      run: () => {
        const a = useActiveEditor.getState().current
        if (a?.projectId) compareWithPicked(a.projectId, a.path)
      },
    },
    {
      id: 'files.showBookmarks',
      title: 'Show Bookmarks',
      group: 'Navigate',
      // Shift+F11 in an editor (its own action); Alt+2 anywhere, where CLion has its Bookmarks window.
      shortcut: 'alt+2',
      keywords: ['bookmark', 'mnemonic', 'marks'],
      icon: BookMarked,
      run: () => useBookmarkPopup.getState().showList(),
    },
    {
      id: 'files.navigateBack',
      title: 'Navigate Back',
      group: 'Navigate',
      shortcut: 'mod+alt+arrowleft',
      keywords: ['back', 'previous location', 'history'],
      icon: ArrowLeft,
      run: () => navigateBack(),
    },
    {
      id: 'files.navigateForward',
      title: 'Navigate Forward',
      group: 'Navigate',
      shortcut: 'mod+alt+arrowright',
      keywords: ['forward', 'next location', 'history'],
      icon: ArrowRight,
      run: () => navigateForward(),
    },
    {
      id: 'files.findInFiles',
      title: 'Find in Files…',
      group: 'Start',
      shortcut: 'mod+shift+f',
      keywords: ['search', 'grep', 'replace'],
      icon: TextSearch,
      when: (c) => !!c.projectId,
      run: (c) => {
        const sel = useActiveEditor.getState().current?.selection() ?? ''
        showSearch(c.projectId, sel && !sel.includes('\n') && sel.length < 200 ? sel : undefined)
      },
    },
    {
      id: 'files.newFile',
      title: 'New File…',
      group: 'Files',
      icon: FilePlus,
      when: (c) => !!c.projectId,
      run: (c) => {
        const a = useActiveEditor.getState().current
        const dir = a && a.projectId === c.projectId ? dirname(a.path) : ''
        void newEntry(c.projectId!, dir === '/' ? '' : dir, 'file')
      },
    },
    {
      id: 'files.newScratch',
      title: 'New Scratch File…',
      group: 'Files',
      shortcut: 'mod+alt+shift+insert',
      keywords: ['scratch', 'notes', 'snippet', 'http request'],
      icon: NotebookPen,
      run: () => chooseScratchKind(),
    },
    {
      id: 'files.showScratches',
      title: 'Show Scratch Files',
      group: 'Files',
      keywords: ['scratches'],
      icon: NotebookPen,
      run: () => {
        useFilesView.getState().setScratches(true)
        showToolWindow('files', 'left')
      },
    },
    {
      id: 'files.revealActive',
      title: 'Reveal Active File in Project Tree',
      group: 'Files',
      shortcut: 'alt+f1',
      icon: Crosshair,
      when: () => !!ctx.projectId,
      run: () => {
        const a = useActiveEditor.getState().current
        if (a) revealInTree(a.projectId, a.path)
        else toast('info', 'No file is open in an editor')
      },
    },
    {
      id: 'files.localHistory',
      title: 'Show Local History',
      group: 'Local History',
      keywords: ['history', 'versions', 'revert', 'undo', 'restore'],
      icon: Clock,
      when: (c) => !!c.projectId,
      run: (c) => {
        const a = useActiveEditor.getState().current
        if (a?.projectId) showLocalHistory(a.projectId, a.path)
        else if (c.projectId) showRecentChanges(c.projectId)
      },
    },
    {
      id: 'files.recentChanges',
      title: 'Recent Changes',
      group: 'Local History',
      shortcut: 'alt+shift+c',
      keywords: ['local history', 'changed files', 'agent edits'],
      icon: ListRestart,
      when: (c) => !!c.projectId,
      run: (c) => c.projectId && showRecentChanges(c.projectId),
    },
    {
      id: 'files.putLabel',
      title: 'Put Label…',
      group: 'Local History',
      keywords: ['local history', 'label', 'checkpoint'],
      icon: Tag,
      when: (c) => !!c.projectId,
      run: (c) => void (c.projectId && putLabel(c.projectId)),
    },
    {
      id: 'files.exportHtml',
      title: 'Export Markdown as HTML…',
      group: 'Files',
      keywords: ['export', 'html', 'markdown', 'share'],
      icon: FileDown,
      when: () => {
        const a = useActiveEditor.getState().current
        return !!a && isMarkdown(a.path)
      },
      run: () => {
        const a = useActiveEditor.getState().current
        if (a && isMarkdown(a.path)) exportAsHtml(a.projectId, a.path)
      },
    },
  ],
  mobileTabs: [{ id: 'files', title: 'Files', icon: FolderTree, order: 30, component: MobileFiles }],
  providers: [FilesProvider],
}

export default feature
