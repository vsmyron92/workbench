// Keeps the apps caches fresh from server events (mounted once as a provider).

import { useEffect, useRef, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useEvent } from '@/api/events'
import { appsKeys, setAppsQueryClient } from './api'
import { applyEnvEvent, applyRunEvent, isActive } from './logic'
import type { EnvHealthEvent, EnvView, RunLive, RunView } from './types'

export function AppsEvents({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  const timers = useRef(new Map<string, number>())

  useEffect(() => {
    setAppsQueryClient(qc)
    const t = timers.current
    return () => {
      setAppsQueryClient(null)
      t.forEach((id) => window.clearTimeout(id))
    }
  }, [qc])

  /** Refetch once things settle (port-in-use flags, service states). */
  const later = (key: readonly unknown[], ms = 800) => {
    const k = JSON.stringify(key)
    window.clearTimeout(timers.current.get(k))
    timers.current.set(
      k,
      window.setTimeout(() => {
        timers.current.delete(k)
        void qc.invalidateQueries({ queryKey: key })
      }, ms),
    )
  }

  useEvent<RunLive & { name: string }>('run.state', (ev) => {
    const pid = ev.projectId
    if (!pid) return
    const key = appsKeys.runs(pid)
    const list = qc.getQueryData<RunView[]>(key)
    if (!list?.some((r) => r.name === ev.data.name)) return later(key, 200)
    qc.setQueryData<RunView[]>(key, list.map((r) => (r.name === ev.data.name ? applyRunEvent(r, ev.data) : r)))
    if (!isActive(ev.data.state)) later(key)
  })

  useEvent<EnvHealthEvent>('env.health', (ev) => {
    const pid = ev.projectId
    if (!pid) return
    const key = appsKeys.envs(pid)
    const list = qc.getQueryData<EnvView[]>(key)
    if (!list) return
    let refetch = false
    const next = list.map((e) => {
      if (e.name !== ev.data.env) return e
      const n = applyEnvEvent(e, ev.data)
      if (!n) refetch = true
      return n ?? e
    })
    qc.setQueryData(key, next)
    if (refetch) later(key, 100)
  })

  useEvent('projects.changed', () => later(['apps'], 300))
  useEvent('resync', () => later(['apps'], 100))

  return <>{children}</>
}
