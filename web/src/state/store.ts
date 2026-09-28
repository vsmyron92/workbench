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

interface UiState {
  projectId: string | null
  sides: Record<Side, SideState>
  prefs: Prefs
  setProject: (id: string | null) => void
  toggleToolWindow: (side: Side, id: string) => void
  showToolWindow: (side: Side, id: string) => void
  hideSide: (side: Side) => void
  setSideSize: (side: Side, size: number) => void
  setPrefs: (p: Partial<Prefs>) => void
}

export const useUi = create<UiState>()(
  persist(
    (set) => ({
      projectId: null,
      sides: {
        left: { active: 'files', size: 300 },
        right: { active: null, size: 380 },
        bottom: { active: null, size: 260 },
      },
      prefs: { theme: 'dark', terminalFontSize: 13, editorFontSize: 13, notifications: true, markdownMode: 'read', editorKeymap: 'clion' },
      setProject: (id) => set({ projectId: id }),
      toggleToolWindow: (side, id) =>
        set((s) => ({ sides: { ...s.sides, [side]: { ...s.sides[side], active: s.sides[side].active === id ? null : id } } })),
      showToolWindow: (side, id) => set((s) => ({ sides: { ...s.sides, [side]: { ...s.sides[side], active: id } } })),
      hideSide: (side) => set((s) => ({ sides: { ...s.sides, [side]: { ...s.sides[side], active: null } } })),
      setSideSize: (side, size) => set((s) => ({ sides: { ...s.sides, [side]: { ...s.sides[side], size } } })),
      setPrefs: (p) => set((s) => ({ prefs: { ...s.prefs, ...p } })),
    }),
    { name: 'wb.ui.v1' },
  ),
)

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
