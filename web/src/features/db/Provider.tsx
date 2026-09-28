// The db slice's provider: hosts the data source dialog. Like every provider it
// wraps the app, so it renders its children.

import type { ReactNode } from 'react'
import { SourceDialogHost } from './SourceDialog'

export function DbProvider({ children }: { children?: ReactNode }) {
  return (
    <>
      {children}
      <SourceDialogHost />
    </>
  )
}
