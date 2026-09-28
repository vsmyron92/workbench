// Scratch files' id and the Files tool window's view (the project, or the scratches),
// kept apart from scratches.ts so the openers can use them without an import cycle.

import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'

/** The hidden project that holds scratch files (server/src/projects.rs SCRATCH_ID). */
export const SCRATCH_ID = 'wb-scratches'

export const isScratch = (projectId: string | null | undefined) => projectId === SCRATCH_ID

/** Whether the Files tool window shows the scratches instead of the project (per browser). */
export const useFilesView = create<{ scratches: boolean; setScratches: (on: boolean) => void }>()(
  persist((set) => ({ scratches: false, setScratches: (scratches) => set({ scratches }) }), {
    name: 'wb.filesView.v1',
    storage: createJSONStorage(() => localStorage),
  }),
)
