// The contract between the shell and feature slices. Each feature exports one
// `FeatureModule` from features/<slice>/index.ts; the shell composes them.

import type { ComponentType, ReactNode } from 'react'
import type { ProjectSummary } from '@/api/types'

/** Props every center-area panel receives. */
export interface PanelProps<P = Record<string, unknown>> {
  /** Stable panel id (e.g. `editor:shop:src/main.rs`). */
  id: string
  params: P
  /** Replace the params (e.g. after navigating inside the panel); persisted in the layout. */
  setParams: (params: P) => void
  setTitle: (title: string) => void
  close: () => void
  /** This panel's tab is the visible one in its group. */
  visible: boolean
  /** This panel's group is the active group and this panel is its visible tab. */
  active: boolean
}

export type PanelComponent = ComponentType<PanelProps<any>>

export interface PanelDef {
  component: PanelComponent
  /** Keep mounted while hidden (terminals, editors). Default false. */
  keepAlive?: boolean
  /** Tab icon. */
  icon?: ComponentType<{ size?: number }>
}

export type Side = 'left' | 'right' | 'bottom'

/** A tool window (CLion-style) docked on a side, toggled from the icon stripe. */
export interface ToolWindowDef {
  id: string
  title: string
  icon: ComponentType<{ size?: number }>
  side: Side
  /** Sort order within the side (lower first). */
  order: number
  component: ComponentType<{ projectId: string | null }>
  /** Small count/dot on the stripe icon (e.g. agents needing attention). */
  badge?: ComponentType<{ projectId: string | null }>
  /** Only show when this returns true for the current project. */
  when?: (project: ProjectSummary | null) => boolean
}

export interface CommandContext {
  projectId: string | null
  project: ProjectSummary | null
}

export interface Command {
  id: string
  title: string
  /** Group heading in the palette. */
  group?: string
  /**
   * e.g. 'mod+p', 'mod+shift+f', 'alt+1'. `mod` = Ctrl (Cmd on macOS). Shortcuts
   * never fire while a terminal has focus, and yield to keys the focused editor binds.
   */
  shortcut?: string
  /** Let `shortcut` fire even inside terminals and editors. Use sparingly. */
  global?: boolean
  /** Extra words that should match in the palette. */
  keywords?: string[]
  icon?: ComponentType<{ size?: number }>
  run: (ctx: CommandContext) => void | Promise<void>
  when?: (ctx: CommandContext) => boolean
}

export interface MobileTabDef {
  id: string
  title: string
  icon: ComponentType<{ size?: number }>
  order: number
  component: ComponentType<{ projectId: string | null }>
  badge?: ComponentType<{ projectId: string | null }>
  /** Only show when this returns true for the current project (like `ToolWindowDef.when`). */
  when?: (project: ProjectSummary | null) => boolean
  /**
   * On a phone, `openPanel` asks each visible tab in turn: return true when this tab can
   * show that panel (after preparing it, e.g. selecting the terminal) and the shell
   * switches to the tab. Panels no tab takes are not opened on a phone.
   */
  openPanel?: (panel: { kind: string; id: string; title?: string; params: Record<string, unknown> }) => boolean
}

/** One result of Search Everywhere (double Shift). */
export interface SearchItem {
  /** Unique within its provider. */
  key: string
  title: string
  /** Character offsets in `title` to emphasize (the matched letters). */
  highlight?: number[]
  /** Muted text after the title (folder, container, file:line). */
  detail?: string
  icon?: ReactNode
  /** Right-aligned hint (a shortcut, a kind). */
  hint?: ReactNode
  /** Open or run it. `side`: Shift+Enter, open to the side where that applies. */
  run: (opts: { side: boolean }) => void
}

/** A tab of Search Everywhere, contributed by a feature. */
export interface SearchProvider {
  id: string
  /** Tab label: "Files", "Symbols"… */
  title: string
  /** Tab and "All" section order (Files 10, Symbols 20, Actions 30, Text 40). */
  order: number
  /** Shortest query `search` runs for (default 1; 0 also runs it on an empty query, e.g. recent files). */
  minQuery?: number
  /** How many results the "All" tab shows for this provider (default 5; 0 keeps it out of All). */
  inAll?: number
  when?: (ctx: CommandContext) => boolean
  /** Results, best first. Called with a debounced query; aborted when it changes. */
  search: (query: string, ctx: CommandContext, signal: AbortSignal) => Promise<SearchItem[]>
  /** A line under the results when this provider cannot answer (e.g. code intelligence is off). */
  hint?: (ctx: CommandContext) => string | null
}

export interface FeatureModule {
  id: string
  /** Center-area panel kinds this feature renders, keyed by kind (see docs/ARCHITECTURE.md#panels). */
  panels?: Record<string, PanelDef>
  toolWindows?: ToolWindowDef[]
  commands?: (ctx: CommandContext) => Command[]
  /** Widgets in the top bar (right of the project switcher), in order. */
  topbar?: ComponentType<{ projectId: string | null }>[]
  /** Widgets in the status bar. */
  statusbar?: ComponentType<{ projectId: string | null }>[]
  mobileTabs?: MobileTabDef[]
  /**
   * Mounted once while signed in (event listeners, global dialogs, keybindings). They
   * wrap the rest of the app, so each must render its `children`: one that returns
   * `null` blanks the whole UI.
   */
  providers?: ComponentType<{ children?: ReactNode }>[]
  /** Tabs of Search Everywhere (double Shift). */
  searchProviders?: SearchProvider[]
}
