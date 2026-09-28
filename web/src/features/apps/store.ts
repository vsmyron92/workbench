// Per-browser preferences of the apps feature (localStorage).

import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { Device } from './logic'

interface AppsPrefs {
  /** Top-bar run selection per project. */
  selected: Record<string, string>
  /** Run groups the user expanded/collapsed (by `<pid>:<group>`). */
  groups: Record<string, boolean>
  device: Device
  select: (pid: string, name: string) => void
  setGroup: (key: string, open: boolean) => void
  setDevice: (d: Device) => void
}

export const useAppsPrefs = create<AppsPrefs>()(
  persist(
    (set) => ({
      selected: {},
      groups: {},
      device: 'desktop',
      select: (pid, name) => set((s) => ({ selected: { ...s.selected, [pid]: name } })),
      setGroup: (key, open) => set((s) => ({ groups: { ...s.groups, [key]: open } })),
      setDevice: (device) => set({ device }),
    }),
    { name: 'wb.apps.v1' },
  ),
)
