// Feature slice: lsp. Owned by the lsp slice — see docs/ARCHITECTURE.md.
// Code intelligence from language servers in the editor: hover, completion,
// diagnostics (Problems), go to declaration / implementation / type, Find Usages,
// rename with preview, file structure, Go to Symbol, formatting, quick fixes.

import { lazy } from 'react'
import { Braces, GitFork, Library, ListTree, LocateFixed, Network, SearchCode, TriangleAlert } from 'lucide-react'
import { showToolWindow, toast } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { fileStructure, showHierarchy } from './actions'
import { lsp } from './client'
import { LspProvider } from './LspProvider'
import { symbolsSearch } from './searchProvider'
import { LspStatusWidget, openLspPopover } from './StatusWidget'
import { usePopups } from './store'

const ProblemsWindow = lazy(() => import('./ProblemsWindow').then((m) => ({ default: m.ProblemsWindow })))
const StructureWindow = lazy(() => import('./StructureWindow').then((m) => ({ default: m.StructureWindow })))
const HierarchyWindow = lazy(() => import('./HierarchyWindow').then((m) => ({ default: m.HierarchyWindow })))
const UsagesWindow = lazy(() => import('./UsagesWindow').then((m) => ({ default: m.UsagesWindow })))
const SourcePanel = lazy(() => import('./SourcePanel').then((m) => ({ default: m.SourcePanel })))

const feature: FeatureModule = {
  id: 'lsp',
  panels: {
    'lsp.source': { component: SourcePanel, keepAlive: true, icon: Library },
  },
  searchProviders: [symbolsSearch],
  toolWindows: [
    { id: 'problems', title: 'Problems', icon: TriangleAlert, side: 'bottom', order: 15, component: ProblemsWindow },
    // Not a magnifier: the left stripe already has Find (in Files) with one.
    { id: 'usages', title: 'Find Usages', icon: LocateFixed, side: 'bottom', order: 17, component: UsagesWindow },
    { id: 'hierarchy', title: 'Hierarchy', icon: Network, side: 'right', order: 50, component: HierarchyWindow },
    { id: 'structure', title: 'Structure', icon: ListTree, side: 'left', order: 18, component: StructureWindow },
  ],
  commands: (ctx) => [
    {
      id: 'lsp.gotoSymbol',
      title: 'Go to Symbol…',
      group: 'Start',
      shortcut: 'mod+alt+shift+n',
      keywords: ['symbol', 'function', 'class', 'type', 'workspace symbol', 'navigate'],
      icon: SearchCode,
      when: (c) => !!c.projectId,
      run: (c) => usePopups.getState().set({ gotoSymbol: { projectId: c.projectId! } }),
    },
    {
      id: 'lsp.showStructure',
      title: 'Show Structure',
      group: 'Tool windows',
      shortcut: 'alt+7',
      keywords: ['outline', 'symbols', 'members'],
      icon: ListTree,
      run: () => showToolWindow('structure', 'left'),
    },
    {
      id: 'lsp.callHierarchy',
      title: 'Call Hierarchy',
      group: 'Code',
      keywords: ['callers', 'callees', 'calls', 'incoming', 'outgoing'],
      icon: Network,
      when: () => !!lsp.lastEditor,
      run: () => {
        const ed = lsp.lastEditor
        if (ed) void showHierarchy(ed, 'call')
        else toast('info', 'No editor is focused')
      },
    },
    {
      id: 'lsp.typeHierarchy',
      title: 'Type Hierarchy',
      group: 'Code',
      keywords: ['supertypes', 'subtypes', 'inheritance', 'implementations', 'subclasses'],
      icon: GitFork,
      when: () => !!lsp.lastEditor,
      run: () => {
        const ed = lsp.lastEditor
        if (ed) void showHierarchy(ed, 'type')
        else toast('info', 'No editor is focused')
      },
    },
    {
      id: 'lsp.fileStructure',
      title: 'File Structure',
      group: 'Code',
      keywords: ['outline', 'symbols', 'members'],
      icon: ListTree,
      when: () => !!lsp.lastEditor,
      run: () => {
        const ed = lsp.lastEditor
        if (ed) void fileStructure(ed)
        else toast('info', 'No editor is focused')
      },
    },
    {
      id: 'lsp.problems',
      title: 'Show Problems',
      group: 'Code',
      shortcut: 'alt+6',
      keywords: ['diagnostics', 'errors', 'warnings'],
      icon: TriangleAlert,
      when: (c) => !!c.projectId,
      run: () => showToolWindow('problems', 'bottom'),
    },
    {
      id: 'lsp.usages',
      title: 'Show Find Usages',
      group: 'Code',
      keywords: ['references', 'usages'],
      icon: LocateFixed,
      when: (c) => !!c.projectId,
      run: () => showToolWindow('usages', 'bottom'),
    },
    {
      id: 'lsp.status',
      title: 'Code Intelligence…',
      group: 'Code',
      keywords: ['language server', 'lsp', 'rust-analyzer', 'restart', 'enable', 'disable'],
      icon: Braces,
      when: (c) => !!c.projectId,
      run: () => openLspPopover(),
    },
    ...(ctx.projectId && lsp.isEnabled(ctx.projectId) === false
      ? [
          {
            id: 'lsp.enable',
            title: 'Enable Code Intelligence…',
            group: 'Code',
            keywords: ['language server', 'lsp'],
            icon: Braces,
            run: () => usePopups.getState().set({ enable: { projectId: ctx.projectId! } }),
          },
        ]
      : []),
  ],
  statusbar: [LspStatusWidget],
  providers: [LspProvider],
}

export default feature
