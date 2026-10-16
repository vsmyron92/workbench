// The data slice's provider: keeps the client state current from the server's events. Like every
// provider it wraps the app, so it renders its children.

import { useEffect, type ReactNode } from 'react'
import { installDataEvents } from './store'

export function DataProvider({ children }: { children?: ReactNode }) {
  useEffect(() => installDataEvents(), [])
  return <>{children}</>
}
