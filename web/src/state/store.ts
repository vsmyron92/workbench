// Client state that is not server data: current project, tool window layout,
// preferences. Persisted per browser in localStorage.

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { Side } from '@/shell/types'

interface SideState {
  /** Visible tool window id, or null when the side is collapsed. */
  active: string | null
  size: number
}

export interface Prefs {
  theme: 'dark' | 'light'
  terminalFontSize: number
  editorFontSize: number
  /** Ask before sending OS notifications. */
  notifications: boolean
  /** How Markdown files open in the editor (absent in older stored prefs). */
  markdownMode?: 'read' | 'split' | 'edit'
  /** Editing keys in the editors: CLion's (the default) or Monaco's own VS Code ones. */
  editorKeymap?: 'clion' | 'vscode'
}

/** The agents window (desktop): its width while the workspace window is open beside it. */
interface ColumnState {
  size: number
}

interface UiState {
  projectId: string | null
  sides: Record<Side, SideState>
  column: ColumnState
  /** The workspace window (stripes, tool windows, the dock) is open beside the agents window; false: the agents window has the whole width. */
  workOpen: boolean
  prefs: Prefs
  setProject: (id: string | null) => void
  setColumn: (c: Partial<ColumnState>) => void
  setWorkOpen: (open: boolean) => void
  toggleToolWindow: (side: Side, id: string) => void
  showToolWindow: (side: Side, id: string) => void
  hideSide: (side: Side) => void
  setSideSize: (side: Side, size: number) => void
  setPrefs: (p: Partial<Prefs>) => void
}

/** A third of the window for the agents column, between 420 and 640 px (an agent CLI wants some 70 columns). */
function defaultColumnSize(): number {
  const width = typeof window === 'undefined' ? 1600 : window.innerWidth
  return Math.round(Math.min(640, Math.max(420, width / 3)))
}

export const useUi = create<UiState>()(
  persist(
    (set) => ({
      projectId: null,
      sides: {
        left: { active: 'workspace', size: 300 },
        right: { active: null, size: 380 },
        bottom: { active: null, size: 260 },
      },
      column: { size: defaultColumnSize() },
      workOpen: true,
      prefs: { theme: 'dark', terminalFontSize: 13, editorFontSize: 13, notifications: true, markdownMode: 'read', editorKeymap: 'clion' },
      setProject: (id) => set({ projectId: id }),
      setColumn: (c) => set((s) => ({ column: { ...s.column, ...c } })),
      setWorkOpen: (workOpen) => set({ workOpen }),
      // Showing a tool window shows the window it lives in.
      toggleToolWindow: (side, id) =>
        set((s) => ({
          sides: { ...s.sides, [side]: { ...s.sides[side], active: s.sides[side].active === id ? null : id } },
          ...(s.sides[side].active !== id ? { workOpen: true } : {}),
        })),
      showToolWindow: (side, id) => set((s) => ({ sides: { ...s.sides, [side]: { ...s.sides[side], active: id } }, workOpen: true })),
      hideSide: (side) => set((s) => ({ sides: { ...s.sides, [side]: { ...s.sides[side], active: null } } })),
      setSideSize: (side, size) => set((s) => ({ sides: { ...s.sides, [side]: { ...s.sides[side], size } } })),
      setPrefs: (p) => set((s) => ({ prefs: { ...s.prefs, ...p } })),
    }),
    {
      name: 'wb.ui.v1',
      version: 1,
      migrate: (persisted, version) => migrateUi(persisted as Partial<UiState>, version) as UiState,
    },
  ),
)

/**
 * Stored state of earlier layouts. Before version 1 agents and terminals were dock tabs
 * and tool windows (`agents` on the left, `terminal` at the bottom); they are a column of
 * their own now, and the Workspace cards come first.
 */
export function migrateUi(state: Partial<UiState>, version: number): Partial<UiState> {
  if (version >= 1 || !state.sides) return state
  const { left, bottom } = state.sides
  return {
    ...state,
    sides: {
      ...state.sides,
      left: { ...left, active: 'workspace' },
      bottom: { ...bottom, active: bottom.active === 'terminal' ? null : bottom.active },
    },
  }
}

/**
 * `data-theme` follows the preference synchronously, when the store changes and
 * before React re-renders: effects that read the CSS variables (xterm, Monaco)
 * then see the new theme, not the previous one.
 */
function applyTheme(theme: Prefs['theme']) {
  if (typeof document !== 'undefined') document.documentElement.dataset.theme = theme
}
applyTheme(useUi.getState().prefs.theme)
useUi.subscribe((s, prev) => {
  if (s.prefs.theme !== prev.prefs.theme) applyTheme(s.prefs.theme)
})

/** Current project id (may be null before projects load). */
export function useProjectId() {
  return useUi((s) => s.projectId)
}
