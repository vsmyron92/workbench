// Client state of the agents feature: dialogs, selected tabs, which terminals are on
// screen (to avoid toasting about a session the user is looking at).

import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export interface NewSessionPrefill {
  prompt?: string
  name?: string
  resume?: string
  fork?: boolean
  provider?: string
}

interface AgentsUi {
  /** Show every project's sessions (agents home and tool window). */
  allProjects: boolean
  /** The agents column's selected tab per project ('' = no project): a terminal id, or null for the home tab. */
  columnTab: Record<string, string | null>
  /**
   * Terminals shown as column tabs on request although they are not the project's open
   * ones (another project's session, a closed one's saved screen), per project.
   */
  columnExtras: Record<string, string[]>
  /** Bumped to put the caret into the home tab's prompt. */
  composerFocus: number
  /** The provider the composer used last (the next session starts with it). */
  lastProvider: string | null
  /** The provider tab of the history list. */
  historyProvider: string | null
  /** Phone: the terminal shown full screen, if any. */
  mobileTerminal: string | null
  dialog: { kind: 'new'; prefill?: NewSessionPrefill } | { kind: 'resume' } | { kind: 'remote' } | null
  setAllProjects: (v: boolean) => void
  selectColumnTab: (project: string, terminalId: string | null) => void
  addColumnExtra: (project: string, terminalId: string) => void
  removeColumnExtra: (project: string, terminalId: string) => void
  focusComposer: () => void
  setLastProvider: (id: string) => void
  setHistoryProvider: (id: string) => void
  setMobileTerminal: (id: string | null) => void
  openDialog: (d: AgentsUi['dialog']) => void
}

export const useAgentsUi = create<AgentsUi>()(
  persist(
    (set) => ({
      allProjects: false,
      columnTab: {},
      columnExtras: {},
      composerFocus: 0,
      lastProvider: null,
      historyProvider: null,
      mobileTerminal: null,
      dialog: null,
      setAllProjects: (allProjects) => set({ allProjects }),
      selectColumnTab: (project, terminalId) =>
        set((s) => (s.columnTab[project] === terminalId ? s : { columnTab: { ...s.columnTab, [project]: terminalId } })),
      addColumnExtra: (project, terminalId) =>
        set((s) => {
          const list = s.columnExtras[project] ?? []
          return list.includes(terminalId) ? s : { columnExtras: { ...s.columnExtras, [project]: [...list, terminalId] } }
        }),
      removeColumnExtra: (project, terminalId) =>
        set((s) => {
          const list = s.columnExtras[project]
          return list?.includes(terminalId) ? { columnExtras: { ...s.columnExtras, [project]: list.filter((x) => x !== terminalId) } } : s
        }),
      focusComposer: () => set((s) => ({ composerFocus: s.composerFocus + 1 })),
      setLastProvider: (lastProvider) => set({ lastProvider }),
      setHistoryProvider: (historyProvider) => set({ historyProvider }),
      setMobileTerminal: (mobileTerminal) => set({ mobileTerminal }),
      openDialog: (dialog) => set({ dialog }),
    }),
    {
      name: 'wb.agents.v1',
      partialize: (s) => ({
        allProjects: s.allProjects,
        columnTab: s.columnTab,
        columnExtras: s.columnExtras,
        lastProvider: s.lastProvider,
        historyProvider: s.historyProvider,
      }),
    },
  ),
)

// Terminals currently rendered and visible, by id (a count: the same terminal can show
// in more than one view at once).
const visible = new Map<string, number>()

export function markVisible(id: string, on: boolean) {
  const n = (visible.get(id) ?? 0) + (on ? 1 : -1)
  if (n > 0) visible.set(id, n)
  else visible.delete(id)
}

export function isTerminalVisible(id: string): boolean {
  return (visible.get(id) ?? 0) > 0 && document.visibilityState === 'visible'
}
