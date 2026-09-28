// Installs the HTTP Client's editor parts (http/editor.ts) once an editor has a
// file open: Monaco is loaded by then, and not loaded for it before.

import { useEffect, type ReactNode } from 'react'
import { hasOpenBuffers, onBuffersChange } from '@/features/files/modelAccess'
import { installHttpEditor } from './editor'

export function HttpProvider({ children }: { children?: ReactNode }) {
  useEffect(() => {
    if (hasOpenBuffers()) {
      void installHttpEditor()
      return
    }
    const off = onBuffersChange(() => {
      if (!hasOpenBuffers()) return
      off()
      void installHttpEditor()
    })
    return off
  }, [])
  // Providers wrap the rest of the app: render it.
  return <>{children}</>
}
