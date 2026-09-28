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
  /** Selected tab of the bottom Terminal tool window, per project. */
  bottomTab: Record<string, string>
  /** The provider the composer used last (the next session starts with it). */
  lastProvider: string | null
  /** The provider tab of the history list. */
  historyProvider: string | null
  /** Phone: the terminal shown full screen, if any. */
  mobileTerminal: string | null
  dialog: { kind: 'new'; prefill?: NewSessionPrefill } | { kind: 'resume' } | { kind: 'remote' } | null
  setAllProjects: (v: boolean) => void
  setBottomTab: (projectId: string, terminalId: string) => void
  setLastProvider: (id: string) => void
  setHistoryProvider: (id: string) => void
  setMobileTerminal: (id: string | null) => void
  openDialog: (d: AgentsUi['dialog']) => void
}

export const useAgentsUi = create<AgentsUi>()(
  persist(
    (set) => ({
      allProjects: false,
      bottomTab: {},
      lastProvider: null,
      historyProvider: null,
      mobileTerminal: null,
      dialog: null,
      setAllProjects: (allProjects) => set({ allProjects }),
      setBottomTab: (projectId, terminalId) => set((s) => ({ bottomTab: { ...s.bottomTab, [projectId]: terminalId } })),
      setLastProvider: (lastProvider) => set({ lastProvider }),
      setHistoryProvider: (historyProvider) => set({ historyProvider }),
      setMobileTerminal: (mobileTerminal) => set({ mobileTerminal }),
      openDialog: (dialog) => set({ dialog }),
    }),
    {
      name: 'wb.agents.v1',
      partialize: (s) => ({ allProjects: s.allProjects, bottomTab: s.bottomTab, lastProvider: s.lastProvider, historyProvider: s.historyProvider }),
    },
  ),
)

// Terminals currently rendered and visible, by id (a count: the same terminal can show
// in a panel and in the bottom tool window at once).
const visible = new Map<string, number>()

export function markVisible(id: string, on: boolean) {
  const n = (visible.get(id) ?? 0) + (on ? 1 : -1)
  if (n > 0) visible.set(id, n)
  else visible.delete(id)
}

export function isTerminalVisible(id: string): boolean {
  return (visible.get(id) ?? 0) > 0 && document.visibilityState === 'visible'
}
