// The always-loaded part of the avatar feature. It keeps the one list (`['avatar']`)
// fresh from the events of the bus, tells the server when a line was not shown because
// avatars are hidden here, answers the agents feature's two questions through the bridge,
// and mounts the overlay (a lazy chunk: looks, the engine and speech load only when an
// avatar is to be shown) inside a boundary of its own.
//
// Like every provider it wraps the app, so it renders its children.

import { lazy, Suspense, useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { subscribe } from '@/api/events'
import { useTerminals } from '@/api/queries'
import { setAvatarShows, setAvatarsOn } from '@/shell/avatarBridge'
import { useUi } from '@/state/store'
import { ErrorBoundary } from '@/ui'
import { avatarApi, useAvatars } from './api'
import { nextLeave, noteExits, presented, sameStage, unacknowledged, whoShows, type Exits, type Staged } from './lib/stage'
import type { AvatarInfo } from './lib/types'
import { setQueryClient } from './queryAccess'
import { emitAct, isStaged, useAvatarRuntime, watchInput } from './runtime'
import { useAvatarPrefs } from './store'
import { LIST_EVENTS, applyEvent } from './sync'
import { sessionOf } from './view'

// Loaded the first time an avatar is to be shown.
const Overlay = lazy(() => import('./Overlay').then((m) => ({ default: m.Overlay })))

/** As many lines are remembered as told to the server while avatars are hidden. */
const MAX_TOLD = 512
const NOBODY: readonly Staged[] = Object.freeze([])

const subscribeVisible = (changed: () => void): (() => void) => {
  document.addEventListener('visibilitychange', changed)
  return () => document.removeEventListener('visibilitychange', changed)
}
const isVisible = (): boolean => document.visibilityState === 'visible'

/**
 * After the overlay failed to render: nothing is shown in its place, and it is tried
 * again when what it shows has changed (another avatar, another design).
 */
function Recover({ reset, signature }: { reset: () => void; signature: string }) {
  const failedWith = useRef(signature)
  useEffect(() => {
    if (failedWith.current !== signature) reset()
  }, [signature, reset])
  return null
}

export function AvatarProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  const { data } = useAvatars()
  const terminals = useTerminals().data
  const projectId = useUi((s) => s.projectId)
  const show = useAvatarPrefs((s) => s.show)
  const visible = useSyncExternalStore(subscribeVisible, isVisible, () => true)

  // Events only change the cache; what they cannot tell is fetched again.
  useEffect(() => {
    const sinks = { act: emitAct, removed: (terminalId: string) => useAvatarPrefs.getState().forget(terminalId) }
    const offs = LIST_EVENTS.map((type) => subscribe(type, (ev) => applyEvent(qc, ev, sinks)))
    return () => offs.forEach((off) => off())
  }, [qc])

  // The palette's commands are listed and run outside React: they read the lists through this.
  useEffect(() => {
    setQueryClient(qc)
    return () => setQueryClient(null)
  }, [qc])

  // The tab used last speaks, and speech needs a first user gesture: both are watched from page load on.
  useEffect(() => watchInput(window), [])

  // What this browser remembers of terminals that were removed while it was closed.
  const pruned = useRef(false)
  useEffect(() => {
    if (!terminals || pruned.current) return
    pruned.current = true
    useAvatarPrefs.getState().prune(terminals.map((t) => t.id))
  }, [terminals])

  // Who is on stage. A sleeping avatar leaves by itself: `tick` looks again then.
  const exits = useRef<Exits>(new Map())
  const [tick, setTick] = useState(0)
  const last = useRef<readonly Staged[]>(NOBODY)
  const staged = useMemo(() => {
    void tick
    if (!data || !terminals || !Array.isArray(data.avatars)) {
      if (last.current.length > 0) last.current = NOBODY
      return last.current
    }
    const now = Date.now()
    const sessions = terminals.map(sessionOf)
    exits.current = noteExits(exits.current, sessions, now)
    const next = whoShows({ enabled: data.enabled === true, avatars: data.avatars, sessions, projectId, exits: exits.current, now })
    // The same array while the same avatars show: the overlay is not told about nothing.
    if (!sameStage(last.current, next)) last.current = next
    return last.current
  }, [data, terminals, projectId, tick])
  useEffect(() => {
    const at = nextLeave(staged)
    if (at === null) return
    const timer = window.setTimeout(() => setTick((n) => n + 1), Math.max(0, at - Date.now()) + 20)
    return () => window.clearTimeout(timer)
  }, [staged])

  const wanted = data?.enabled === true && show && staged.length > 0

  // What the rest of the app may ask: which avatars are shown, whether one is on screen, which sessions have one.
  useEffect(() => {
    useAvatarRuntime.getState().tell({ shown: wanted ? staged.map((s) => s.terminalId) : [] })
  }, [wanted, staged])
  useEffect(() => {
    setAvatarShows(isStaged)
    return () => {
      setAvatarShows(null)
      setAvatarsOn(null)
      useAvatarRuntime.getState().tell({ shown: [] })
    }
  }, [])
  useEffect(() => {
    if (!data || !Array.isArray(data.avatars)) setAvatarsOn(null)
    else setAvatarsOn(new Set(data.enabled === true ? data.avatars.filter((a) => a?.on === true).map((a) => a.terminalId) : []))
  }, [data])

  // Hidden means silent: a visible tab that hides its avatars says so for every line it would have presented.
  const told = useRef(new Set<string>())
  useEffect(() => {
    if (!data || data.enabled !== true || show || !visible || staged.length === 0) return
    const byId = new Map<string, AvatarInfo>()
    for (const a of data.avatars) byId.set(a.terminalId, a)
    const records: AvatarInfo[] = []
    for (const s of staged) {
      const a = byId.get(s.terminalId)
      if (a) records.push(presented(a, s.foreign))
    }
    for (const line of unacknowledged(records, told.current)) {
      told.current.add(line.key)
      // A line that is gone meanwhile, or a request that fails: the server calls it unheard by itself.
      avatarApi.heard(line.terminalId, line.lineId, 'hidden').catch(() => {})
    }
    if (told.current.size > MAX_TOLD) told.current = new Set(Array.from(told.current).slice(-MAX_TOLD / 2))
  }, [data, show, visible, staged])

  const signature = useMemo(() => {
    const revisions = new Map<string, number>()
    for (const a of data?.avatars ?? []) revisions.set(a.terminalId, a.revision)
    return staged.map((s) => `${s.terminalId}:${revisions.get(s.terminalId) ?? ''}`).join(',')
  }, [data, staged])

  return (
    <>
      {children}
      {wanted && (
        <ErrorBoundary label="avatar overlay" fallback={(_error, reset) => <Recover reset={reset} signature={signature} />}>
          <Suspense fallback={null}>
            <Overlay staged={staged} />
          </Suspense>
        </ErrorBoundary>
      )}
    </>
  )
}

/**
 * The feature's widget in the desktop shell's top bar: the place of the compact face.
 * It takes no room itself; the overlay puts the face into it while the workspace window
 * is collapsed.
 */
export function AvatarBarSlot() {
  const setBarSlot = useAvatarRuntime((s) => s.setBarSlot)
  return <span ref={setBarSlot} style={{ display: 'contents' }} />
}
