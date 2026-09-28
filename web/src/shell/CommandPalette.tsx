// Ctrl+K (Ctrl+Shift+P from anywhere) command palette, plus the keyboard shortcut
// dispatcher for every command that declares `shortcut`.

import { useEffect, useMemo, useState } from 'react'
import { Command as Cmdk } from 'cmdk'
import { create } from 'zustand'
import { FolderGit2, FolderPlus, Moon, PanelLeft, RefreshCw, Search, Sun } from 'lucide-react'
import { api } from '@/api/client'
import { useProjects } from '@/api/queries'
import { useUi } from '@/state/store'
import { Kbd } from '@/ui'
import { featureCommands, toolWindows } from './registry'
import { addProjectInteractive, toast } from './actions'
import { openSearchEverywhere } from './searchEverywhereStore'
import { visibleFor } from './visibility'
import { GLOBAL_PALETTE_SHORTCUT, dedupeByTitle, isMacPlatform, matchShortcut, rankCommands, shortcutsMayHandle } from './paletteSearch'
import type { Command, CommandContext } from './types'

const usePalette = create<{ open: boolean; set: (o: boolean) => void }>()((set) => ({ open: false, set: (open) => set({ open }) }))

export function openPalette() {
  usePalette.getState().set(true)
}

export function useCommandContext(): CommandContext {
  const projectId = useUi((s) => s.projectId)
  const { data } = useProjects()
  const project = data?.find((p) => p.id === projectId) ?? null
  return { projectId, project }
}

function coreCommands(ctx: CommandContext, projects: { id: string; name: string }[]): Command[] {
  const ui = useUi.getState()
  const cmds: Command[] = [
    {
      id: 'core.searchEverywhere',
      title: 'Search Everywhere (Shift Shift)',
      group: 'Start',
      icon: Search,
      keywords: ['double shift', 'find', 'navigate', 'symbol', 'file', 'action'],
      run: () => openSearchEverywhere(),
    },
    {
      id: 'core.reloadProjects',
      title: 'Reload projects',
      group: 'Workbench',
      icon: RefreshCw,
      run: async () => {
        await api.post('/api/projects/reload')
        toast('success', 'Projects reloaded')
      },
    },
    {
      id: 'core.addProject',
      title: 'Add project…',
      group: 'Workbench',
      icon: FolderPlus,
      keywords: ['project', 'open folder', 'directory', 'repository'],
      run: () => addProjectInteractive(),
    },
    {
      id: 'core.toggleTheme',
      title: ui.prefs.theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme',
      group: 'Workbench',
      icon: ui.prefs.theme === 'dark' ? Sun : Moon,
      run: () => useUi.getState().setPrefs({ theme: useUi.getState().prefs.theme === 'dark' ? 'light' : 'dark' }),
    },
    // Only the tool windows this project shows ("Show GitHub" on a GitLab project would
    // close the open window and show nothing).
    ...visibleFor(toolWindows, ctx.project).map<Command>((t) => ({
      id: `core.toolWindow.${t.id}`,
      title: `Show ${t.title}`,
      group: 'Tool windows',
      icon: t.icon ?? PanelLeft,
      run: () => useUi.getState().showToolWindow(t.side, t.id),
    })),
    ...projects
      .filter((p) => p.id !== ctx.projectId)
      .map<Command>((p) => ({
        id: `core.project.${p.id}`,
        title: `Switch to project ${p.name}`,
        group: 'Projects',
        icon: FolderGit2,
        keywords: ['project', p.id],
        run: () => useUi.getState().setProject(p.id),
      })),
  ]
  return cmds
}

/** Every palette command for this context (features first, then the generated ones). */
export function paletteCommands(ctx: CommandContext, projects: { id: string; name: string }[]): Command[] {
  return dedupeByTitle(featureCommands(ctx), coreCommands(ctx, projects))
}

export function formatShortcut(s: string): string {
  const isMac = isMacPlatform()
  return s
    .split('+')
    .map((p) => (p === 'mod' ? (isMac ? '⌘' : 'Ctrl') : p === 'shift' ? 'Shift' : p === 'alt' ? (isMac ? '⌥' : 'Alt') : p.length === 1 ? p.toUpperCase() : p))
    .join('+')
}

/** Mounted once by the desktop shell. */
export function CommandPalette() {
  const { open, set } = usePalette()
  const ctx = useCommandContext()
  const { data: projects } = useProjects()
  const [query, setQuery] = useState('')

  const commands = useMemo(
    () => paletteCommands(ctx, projects ?? []),
    // recompute when the palette opens or the project changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [open, ctx.projectId, projects],
  )

  useEffect(() => {
    const togglePalette = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()
      set(!usePalette.getState().open)
    }
    // Capture phase: only what must work everywhere (terminals and editors included).
    const onCapture = (e: KeyboardEvent) => {
      if (matchShortcut(e, GLOBAL_PALETTE_SHORTCUT)) return togglePalette(e)
      for (const c of commands) {
        if (c.global && c.shortcut && matchShortcut(e, c.shortcut)) {
          e.preventDefault()
          e.stopPropagation()
          void c.run(ctx)
          return
        }
      }
    }
    // Bubble phase: after the focused widget. xterm and Monaco stop the keys they
    // use, and a terminal keeps every key (see shortcutsMayHandle).
    const onBubble = (e: KeyboardEvent) => {
      if (!shortcutsMayHandle(e)) return
      if (matchShortcut(e, 'mod+k')) return togglePalette(e)
      for (const c of commands) {
        if (!c.global && c.shortcut && matchShortcut(e, c.shortcut)) {
          e.preventDefault()
          void c.run(ctx)
          return
        }
      }
    }
    window.addEventListener('keydown', onCapture, true)
    window.addEventListener('keydown', onBubble)
    return () => {
      window.removeEventListener('keydown', onCapture, true)
      window.removeEventListener('keydown', onBubble)
    }
  }, [commands, ctx, set])

  useEffect(() => {
    if (!open) setQuery('')
  }, [open])

  const groups = useMemo(() => {
    const m = new Map<string, Command[]>()
    for (const c of commands) {
      const g = c.group ?? 'Commands'
      if (!m.has(g)) m.set(g, [])
      m.get(g)!.push(c)
    }
    return [...m.entries()]
  }, [commands])
  // While searching: one flat list ranked by relevance (cmdk cannot reorder groups).
  const ranked = useMemo(() => (query.trim() ? rankCommands(commands, query) : null), [commands, query])

  const item = (c: Command) => (
    <Cmdk.Item
      key={c.id}
      value={c.id}
      onSelect={() => {
        set(false)
        void c.run(ctx)
      }}
    >
      {c.icon ? <c.icon size={15} /> : <span style={{ width: 15 }} />}
      <span className="wb-grow wb-ellipsis">{c.title}</span>
      {ranked && c.group && <span className="wb-muted wb-small">{c.group}</span>}
      {c.shortcut && <Kbd>{formatShortcut(c.shortcut)}</Kbd>}
    </Cmdk.Item>
  )

  return (
    <Cmdk.Dialog
      open={open}
      onOpenChange={set}
      label="Command palette"
      className="wb-palette"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
    >
      <Cmdk.Input value={query} onValueChange={setQuery} placeholder="Type a command…" autoFocus />
      <Cmdk.List>
        <Cmdk.Empty>No matching commands.</Cmdk.Empty>
        {ranked
          ? ranked.map(item)
          : groups.map(([g, cmds]) => (
              <Cmdk.Group key={g} heading={g}>
                {cmds.map(item)}
              </Cmdk.Group>
            ))}
      </Cmdk.List>
    </Cmdk.Dialog>
  )
}
