// Which cards the user allowed to read live project data (per browser, localStorage).

import { create } from 'zustand'
import { persist } from 'zustand/middleware'

interface BridgePrefs {
  /** `<scope>/<card id>` of the cards allowed. */
  allowed: Record<string, true>
  allow: (key: string) => void
  revoke: (key: string) => void
}

export const useBridgePrefs = create<BridgePrefs>()(
  persist(
    (set) => ({
      allowed: {},
      allow: (key) => set((s) => ({ allowed: { ...s.allowed, [key]: true } })),
      revoke: (key) =>
        set((s) => {
          const { [key]: _gone, ...rest } = s.allowed
          return { allowed: rest }
        }),
    }),
    { name: 'wb.report-bridge.v1' },
  ),
)

export const bridgeKey = (scope: string, cardId: string) => `${scope}/${cardId}`
