// Which tab says a line, and what it tells the server afterwards.
//
// Every open Workbench tab learns of every agent line. Each presents it (a caption costs
// nothing), but only one may say it aloud: the tab that holds the server's lease for the
// line. A tab asks for the lease only if it could speak right now, and waits a moment
// first, shorter the more recently it was used, so the tab the user is at wins and keeps
// winning. The tab that says a line renews its lease while it says it, every few seconds and
// as each chunk starts; a lease lapses by itself once nobody renews it, and a tab that goes
// away gives its leases back.
//
// The arbiter feeds the window's speaker and acknowledges what became of each line
// (`spoken`, `skipped`, `failed`, `shown`). It never compares its clock with a line's time:
// what is stale is the server's decision alone.
//
// Two things need no lease. A tab that hides its avatars presents nothing at all, aloud or
// as a caption. And a line the user asks this tab to say (`replay`: "Hear it" on a line
// that was only shown) is said here whoever held its lease.
//
// Pure TypeScript: the clock, timers, visibility and the two requests are passed in.

import type { Speaker } from './speech'
import type { AvatarInfo, Line, Outcome, Voice } from './types'

/** The key a line is known by in this tab. Line ids never repeat; the time is part of it all the same. */
export function lineKey(terminalId: string, line: Pick<Line, 'id' | 'at'>): string {
  return `${terminalId}:${line.id}:${line.at}`
}

/**
 * How long a tab waits before it asks for a line. A hidden tab: 900 ms. A visible one:
 * a two-hundredth of the time since its last pointer or key input, at most 600 ms, so the
 * tab used last asks first.
 */
export function claimDelayMs(visible: boolean, idleMs: number): number {
  if (!visible) return 900
  if (typeof idleMs !== 'number' || Number.isNaN(idleMs)) return 600
  return Math.min(600, Math.max(0, idleMs / 200))
}

export interface ArbiterHost {
  /** This page load's id: 12 random bytes, base64url. */
  tab: string
  now(): number
  setTimeout(cb: () => void, ms: number): number
  clearTimeout(id: number): void
  visible(): boolean
  /** Time since the last pointer or key input in this tab. */
  idleMs(): number
  claim(terminalId: string, lineId: string, body: { tab: string; release?: true }): Promise<{ speak: boolean; leaseMs: number; retryMs: number }>
  heard(terminalId: string, lineId: string, outcome: Outcome, voice?: string): Promise<void>
}

export interface ArbiterState {
  /** The user chose voice in this browser. */
  voice: boolean
  /** Speech is unlocked in this page load. */
  unlocked: boolean
  /**
   * Avatars are shown in this browser. While false nothing is presented here: no caption
   * and no speech, and no line is asked for (the provider acknowledges `hidden`).
   */
  show: boolean
  /** Whether this tab presents this avatar's lines at all (its session is of the project on screen, or it has an open ask). */
  shown(terminalId: string): boolean
  /** The session needs a permission or an answer: its avatar is quiet. */
  blocked(terminalId: string): boolean
  /** The session on screen: its lines go before others'. */
  onScreen(terminalId: string): boolean
}

export interface Arbiter {
  /** An avatar's record as the cache holds it now. Every agent line without an outcome that was not seen before is presented. */
  offer(avatar: AvatarInfo): void
  /** A line got an outcome (from any device). A line this tab is saying stops when that outcome is `skipped`. */
  settled(terminalId: string, line: Line): void
  /** The avatar is gone: its lines stop here and nothing more is acknowledged for it. */
  forget(terminalId: string): void
  /**
   * The user asked this tab to say a line it has, or had, on screen ("Hear it"). The line
   * goes to the speaker in mode `voice` without a claim: no other tab is asked, and it
   * goes before everything that waits. Its caption, if it is on screen now, starts over
   * aloud. What becomes of it is acknowledged like any other line (`spoken`, `skipped`,
   * `failed`), also after an earlier `shown` or `skipped`. Call it after the speaker has its
   * new preferences. false: nothing was queued (the avatar is hidden or off, its session
   * needs the user, or `line` is no agent line).
   */
  replay(avatar: AvatarInfo, line: Line): boolean
  setState(s: ArbiterState): void
  /** The page is going away: speech stops at once and every lease is given back. Nothing else is posted. */
  release(): void
  dispose(): void
}

/** How long a claim may take before the line is presented silently. A late yes is given back. */
export const CLAIM_TIMEOUT_MS = 4000
/** A lease this fresh is not renewed when its line reaches the head of the queue, or when a chunk of it starts. */
export const RENEW_FRESH_MS = 2000
/**
 * While a line is said, its lease is renewed this long after it was last renewed, or half
 * the lease the server gave, whichever is sooner (never sooner than `RENEW_FRESH_MS`). One
 * chunk can take half a minute in a slow voice, and the server's lease (at least 15 s) is an
 * estimate that knows neither the voice nor the speed. A renewal that got no answer is asked
 * for again `RENEW_FRESH_MS` later: half the lease would leave no time for a second failure.
 */
export const RENEW_EVERY_MS = 10_000
const renewEvery = (leaseMs: number): number => (leaseMs > 0 ? Math.max(RENEW_FRESH_MS, Math.min(RENEW_EVERY_MS, leaseMs / 2)) : RENEW_EVERY_MS)
const leaseOf = (answer: { leaseMs?: unknown } | null | undefined): number => (typeof answer?.leaseMs === 'number' && Number.isFinite(answer.leaseMs) ? answer.leaseMs : 0)
const RETRY_MARGIN_MS = 50
const MAX_RETRY_MS = 120_000
const MAX_TERMINALS = 128

/** Best first, as the server ranks them. */
const RANK: ReadonlyMap<string, number> = new Map([
  ['spoken', 5],
  ['skipped', 4],
  ['failed', 3],
  ['shown', 2],
  ['hidden', 1],
  ['unheard', 0],
])
const OUTCOMES: ReadonlyMap<string, Outcome> = new Map<string, Outcome>([
  ['spoken', 'spoken'],
  ['skipped', 'skipped'],
  ['failed', 'failed'],
  ['shown', 'shown'],
  ['hidden', 'hidden'],
  ['unheard', 'unheard'],
])
const rank = (o: Outcome | null): number => (o === null ? -1 : (RANK.get(o) ?? -1))

/**
 * `waiting`: the claim delay runs. `claiming`: the claim is with the server. `queued`: the
 * speaker has it. `presenting`: the speaker reported its start. `done`: over, and
 * remembered so the line is not presented twice.
 */
type Phase = 'waiting' | 'claiming' | 'queued' | 'presenting' | 'done'

interface Entry {
  key: string
  terminalId: string
  lineId: string
  text: string
  lang: string
  voice: Voice
  /** An ask's question: it goes before other lines. */
  ask: boolean
  phase: Phase
  /** The claim delay or the claim's timeout; while this tab says the line, the next renewal of its lease. */
  timer: number | null
  /** Bumped whenever a claim in flight is given up: its answer is then late. */
  attempt: number
  /** This tab holds the lease, as far as it knows. */
  leased: boolean
  leasedAt: number
  /** How long the server said the lease lasts, as it last said; 0: not known. */
  leaseMs: number
  /** The last renewal of its lease got no answer (the request failed): the next one is asked for soon. */
  renewFailed: boolean
  /** The best outcome known, from the server or from this tab's own acknowledgement. */
  outcome: Outcome | null
  /** How the speaker was given it. */
  mode: 'voice' | 'silent' | null
  /** The speaker has had it once: it is never queued a second time as a caption. */
  presented: boolean
  /** Do not acknowledge what the speaker reports for it: the stop did not come from the user here. */
  quiet: boolean
  /** It was stopped because its session needs the user: that is a caption shown, not a line skipped. */
  blockedStop: boolean
  /** It is being put back into the queue: the `end` of its old place is not its end. */
  requeue: boolean
  /** The user asked this tab to say it (`replay`): it needs no lease and goes before what waits. */
  byUser: boolean
  /** The browser voice that says it; null while it is a caption. */
  voiceName: string | null
  retried: boolean
}

/** A line the user asked for goes before an ask's question and the session on screen (which add up to 3). */
const BY_USER_PRIORITY = 4

/**
 * The window's arbiter. It listens to the speaker (`speaker.subscribe`) to learn what
 * became of each line it queued.
 */
export function createArbiter(host: ArbiterHost, speaker: Speaker, state: ArbiterState): Arbiter {
  /** Terminal -> line key -> what this tab did with the line. An entry lives as long as the record lists its line, or it is in flight. */
  const terminals = new Map<string, Map<string, Entry>>()
  /** The record last offered for each terminal, so a change of state can look at its lines again. */
  const offered = new Map<string, AvatarInfo>()
  /** The entries the speaker has, by key: how its events find them. */
  const queued = new Map<string, Entry>()
  const timers = new Set<number>()
  let current = state
  let disposed = false

  const flag = (read: () => boolean): boolean => {
    try {
      return read() === true
    } catch {
      return false
    }
  }
  /** This tab presents this avatar's lines: avatars are shown here, and this one is among them. Hidden means silent. */
  const shows = (tid: string): boolean => current.show === true && flag(() => current.shown(tid))
  const blocked = (tid: string): boolean => flag(() => current.blocked(tid))
  const maySpeak = (tid: string): boolean => current.voice === true && current.unlocked === true && shows(tid) && !blocked(tid)
  const visible = (): boolean => flag(() => host.visible())
  const now = (): number => {
    const t = host.now()
    return typeof t === 'number' && Number.isFinite(t) ? t : 0
  }
  const inFlight = (entry: Entry): boolean => entry.phase !== 'done'

  const later = (cb: () => void, ms: number): number => {
    const id = host.setTimeout(() => {
      timers.delete(id)
      if (!disposed) cb()
    }, Math.max(0, ms))
    timers.add(id)
    return id
  }
  const cancel = (entry: Entry) => {
    if (entry.timer === null) return
    timers.delete(entry.timer)
    host.clearTimeout(entry.timer)
    entry.timer = null
  }

  /** Gives a line's lease back to the server, whoever asks: the one request that is still sent while the page goes away. */
  const releaseLine = (terminalId: string, lineId: string) => {
    try {
      host.claim(terminalId, lineId, { tab: host.tab, release: true }).catch(() => {})
    } catch {
      // The lease lapses by itself.
    }
  }
  const giveBack = (entry: Entry) => {
    // Its renewal ends with the lease (while the line is presented, the timer is the renewal's).
    if (entry.phase === 'presenting') cancel(entry)
    if (!entry.leased) return
    entry.leased = false
    releaseLine(entry.terminalId, entry.lineId)
  }

  /** Tells the server what became of a line, unless it already knows something at least as good. */
  const acknowledge = (entry: Entry, outcome: Outcome, voice?: string | null) => {
    if (rank(outcome) <= rank(entry.outcome)) return
    entry.outcome = outcome
    try {
      host.heard(entry.terminalId, entry.lineId, outcome, voice ?? undefined).catch(() => {})
    } catch {
      // Not acknowledged then: the server calls the line unheard once it is stale.
    }
  }

  /** The line is over for this tab. A claim still in flight becomes late. */
  const done = (entry: Entry) => {
    cancel(entry)
    entry.attempt++
    entry.phase = 'done'
    entry.byUser = false
    queued.delete(entry.key)
  }

  const priorityOf = (entry: Entry): number => (entry.byUser ? BY_USER_PRIORITY : 0) + (entry.ask ? 2 : 0) + (flag(() => current.onScreen(entry.terminalId)) ? 1 : 0)

  /** Asked by the speaker when a line this tab may say reaches the head of the queue: the lease, renewed unless it is fresh. */
  const confirm = (entry: Entry): Promise<boolean> => {
    if (disposed || !inFlight(entry) || !maySpeak(entry.terminalId)) {
      giveBack(entry)
      return Promise.resolve(false)
    }
    if (entry.leased && now() - entry.leasedAt < RENEW_FRESH_MS) return Promise.resolve(true)
    let asked: Promise<{ speak: boolean; leaseMs: number; retryMs: number }>
    try {
      asked = host.claim(entry.terminalId, entry.lineId, { tab: host.tab })
    } catch {
      entry.leased = false
      return Promise.resolve(false)
    }
    return asked.then(
      (answer) => {
        entry.leased = answer?.speak === true
        entry.leasedAt = now()
        if (entry.leased) {
          entry.leaseMs = leaseOf(answer)
          entry.renewFailed = false
        }
        // Renewed for a line that is no longer wanted: the lease goes straight back.
        if (entry.leased && (disposed || !inFlight(entry))) giveBack(entry)
        return entry.leased
      },
      () => {
        entry.leased = false
        return false
      },
    )
  }

  const present = (entry: Entry, mode: 'voice' | 'silent') => {
    cancel(entry)
    entry.phase = 'queued'
    entry.mode = mode
    entry.presented = true
    queued.set(entry.key, entry)
    speaker.enqueue({
      key: entry.key,
      terminalId: entry.terminalId,
      lineId: entry.lineId,
      text: entry.text,
      lang: entry.lang,
      voice: entry.voice,
      seed: entry.terminalId,
      mode,
      priority: priorityOf(entry),
      // A line the user asked this tab for is said without a lease, so there is none to renew.
      confirm: mode === 'voice' && !entry.byUser ? () => confirm(entry) : undefined,
    })
  }

  /** The line cannot be said here: its caption, unless the speaker has had it already. */
  const caption = (entry: Entry) => {
    if (entry.presented) done(entry)
    else present(entry, 'silent')
  }

  const claim = (entry: Entry) => {
    entry.timer = null
    if (entry.phase !== 'waiting') return
    if (!maySpeak(entry.terminalId)) {
      if (entry.presented) done(entry)
      else look(entry)
      return
    }
    entry.phase = 'claiming'
    const attempt = ++entry.attempt
    const late = () => disposed || attempt !== entry.attempt || entry.phase !== 'claiming'
    entry.timer = later(() => {
      // No answer: the caption does not wait for it. A yes that still comes is given back.
      if (late()) return
      entry.timer = null
      entry.attempt++
      caption(entry)
    }, CLAIM_TIMEOUT_MS)
    let asked: Promise<{ speak: boolean; leaseMs: number; retryMs: number }>
    try {
      asked = host.claim(entry.terminalId, entry.lineId, { tab: host.tab })
    } catch {
      entry.attempt++
      caption(entry)
      return
    }
    asked.then(
      (answer) => {
        const speak = answer?.speak === true
        if (late()) {
          if (speak) releaseLine(entry.terminalId, entry.lineId)
          return
        }
        cancel(entry)
        if (speak) {
          entry.leased = true
          entry.leasedAt = now()
          entry.leaseMs = leaseOf(answer)
          entry.renewFailed = false
          if (maySpeak(entry.terminalId)) {
            present(entry, 'voice')
          } else {
            giveBack(entry)
            caption(entry)
          }
          return
        }
        // Another tab has it: the caption here, and one more try when that lease has run out.
        const retryMs = typeof answer?.retryMs === 'number' && Number.isFinite(answer.retryMs) ? answer.retryMs : 0
        const again = retryMs > 0 && !entry.retried
        caption(entry)
        if (again) {
          entry.retried = true
          later(() => retry(entry), Math.min(retryMs, MAX_RETRY_MS) + RETRY_MARGIN_MS)
        }
      },
      () => {
        if (late()) return
        caption(entry)
      },
    )
  }

  /** The other tab's lease has run out. If the line still has no outcome at all, this tab may say it after all. */
  const retry = (entry: Entry) => {
    if (inFlight(entry) || entry.outcome !== null || terminals.get(entry.terminalId)?.get(entry.key) !== entry) return
    if (!maySpeak(entry.terminalId)) return
    entry.phase = 'waiting'
    claim(entry)
  }

  /** A line that is not in the speaker's hands: ask for it, show it, or leave it for later. */
  const look = (entry: Entry) => {
    cancel(entry)
    const tid = entry.terminalId
    if (!shows(tid)) {
      // Not presented here now. Forgotten, so it is looked at again once this avatar is shown.
      done(entry)
      if (terminals.get(tid)?.get(entry.key) === entry) terminals.get(tid)?.delete(entry.key)
      return
    }
    if (!maySpeak(tid)) {
      caption(entry)
      return
    }
    entry.phase = 'waiting'
    entry.timer = later(() => claim(entry), claimDelayMs(visible(), host.idleMs()))
  }

  /**
   * An outcome arrived for a line: stop what it makes pointless. Only news counts: the
   * record repeats what is known with every change of the cache, and a line the user asked
   * to hear again already has an outcome while it is said.
   */
  const settle = (entry: Entry, outcome: Outcome) => {
    if (rank(outcome) <= rank(entry.outcome)) return
    entry.outcome = outcome
    if (!inFlight(entry)) return
    // Skipped on some device, or stale: it stops here too, and this tab adds nothing.
    const over = outcome === 'skipped' || outcome === 'unheard'
    if (entry.phase === 'waiting' || entry.phase === 'claiming') {
      if (over) {
        giveBack(entry)
        done(entry)
      } else if (outcome === 'spoken' || outcome === 'failed') {
        // Another tab had the lease and is through with it: only the caption is left to show.
        entry.attempt++
        caption(entry)
      }
      return
    }
    if (!over) return
    entry.quiet = true
    giveBack(entry)
    speaker.stop(entry.key)
    // A speaker that no longer answers (it was disposed first) must not keep the line in flight for ever.
    if (inFlight(entry)) done(entry)
  }

  /** This tab says the line under a lease of its own, and keeps it while it does. */
  const holds = (entry: Entry): boolean => !disposed && inFlight(entry) && entry.leased && !entry.byUser && entry.voiceName !== null

  /** Sets the next renewal of the lease of a line this tab says: `renewEvery` after the last one, or soon after one that failed. */
  const renewLater = (entry: Entry) => {
    cancel(entry)
    if (!holds(entry)) return
    entry.timer = later(
      () => {
        entry.timer = null
        renew(entry)
      },
      entry.leasedAt + (entry.renewFailed ? RENEW_FRESH_MS : renewEvery(entry.leaseMs)) - now(),
    )
  }

  /**
   * Renews the lease of a line this tab says, unless it is fresh, and sets the next renewal.
   * The server's lease is an estimate of the speech time that knows neither the voice nor the
   * speed: renewed only when the line reached the head of the queue, it could lapse while the
   * line is still said, and another tab would then take it and say the line again. So it is
   * renewed as each chunk starts, and on a timer while a chunk takes long.
   */
  const renew = (entry: Entry) => {
    if (!holds(entry)) return
    if (now() - entry.leasedAt >= RENEW_FRESH_MS) askAgain(entry)
    renewLater(entry)
  }

  /**
   * Asks the server to renew the lease. The time of the request counts as the renewal, so a
   * network that keeps failing is asked every `RENEW_FRESH_MS` at most, never in a rush.
   */
  const askAgain = (entry: Entry) => {
    entry.leasedAt = now()
    let asked: Promise<{ speak: boolean; leaseMs: number; retryMs: number }>
    try {
      asked = host.claim(entry.terminalId, entry.lineId, { tab: host.tab })
    } catch {
      // The lease runs for as long as it was given; the next renewal asks again soon.
      entry.renewFailed = true
      return
    }
    asked.then(
      (answer) => {
        entry.leased = answer?.speak === true
        if (entry.leased) {
          entry.leaseMs = leaseOf(answer)
          // Renewed for a line that is no longer said here: the lease goes straight back.
          if (disposed || !inFlight(entry) || entry.voiceName === null) {
            giveBack(entry)
            return
          }
          // Renewed after a failure: the next renewal is a whole interval away again.
          const failed = entry.renewFailed
          entry.renewFailed = false
          if (failed && entry.phase === 'presenting') renewLater(entry)
          return
        }
        // Not renewed: nothing renews it any more.
        if (entry.phase === 'presenting') cancel(entry)
        // A no with a time to try again: the lease had lapsed and the server gave the line to
        // another tab, which says it now. This tab falls silent rather than talk over it, and
        // leaves the outcome to that tab. A plain no takes the line from nobody (it is over,
        // stale, or its avatar is off): what follows from that comes with the record.
        const retryMs = answer?.retryMs
        if (typeof retryMs === 'number' && retryMs > 0 && inFlight(entry) && !entry.byUser && entry.voiceName !== null) silence(entry)
      },
      () => {
        // Not renewed this time; the next renewal asks again soon, while there is time left on the lease.
        entry.renewFailed = true
        if (entry.phase === 'presenting') renewLater(entry)
      },
    )
  }

  const unsubscribe = speaker.subscribe({
    start(key, engine, _chunks, voiceName) {
      const entry = queued.get(key)
      if (!entry || disposed) return
      entry.phase = 'presenting'
      entry.voiceName = engine === 'browser' ? voiceName : null
      if (engine !== 'silent') {
        // Said under the lease: it is renewed while it is said.
        renewLater(entry)
        return
      }
      // A caption needs no lease: another tab may want it.
      giveBack(entry)
      if (!entry.quiet && visible()) acknowledge(entry, 'shown')
    },
    /** A chunk of a line this tab says under a lease: the lease is renewed, unless it is fresh. */
    chunk(key) {
      const entry = queued.get(key)
      if (entry) renew(entry)
    },
    end(key, outcome) {
      const entry = queued.get(key)
      if (!entry || disposed || entry.requeue) return
      const quiet = entry.quiet
      const blockedStop = entry.blockedStop
      const voiceName = entry.voiceName
      done(entry)
      if (!quiet) {
        if (outcome === 'spoken') {
          // A line that was skipped while it was said here is quiet by now, so this is never called
          // spoken after such a skip. A line the user asked to hear after a skip is: it was said.
          acknowledge(entry, 'spoken', voiceName)
        } else if (outcome === 'failed') {
          acknowledge(entry, 'failed')
        } else if (outcome === 'skipped' && !blockedStop) {
          acknowledge(entry, 'skipped')
        } else if (visible()) {
          acknowledge(entry, 'shown')
        }
      }
      // An acknowledged line needs no lease any more; one that was never acknowledged gives it back.
      if (entry.outcome === null || quiet) giveBack(entry)
      else entry.leased = false
    },
  })

  /** Stops a line here without a word to the server, wherever it is. */
  const silence = (entry: Entry) => {
    if (!inFlight(entry)) return
    const held = entry.phase === 'queued' || entry.phase === 'presenting'
    entry.quiet = true
    giveBack(entry)
    done(entry)
    if (held) speaker.stop(entry.key)
  }

  /**
   * The same for several lines at once. All of them are marked first: stopping one lets the
   * speaker start the next, and that one must not be acknowledged on its way out.
   */
  const silenceAll = (entries: Entry[]) => {
    for (const entry of entries) entry.quiet = true
    for (const entry of entries) silence(entry)
  }

  /** At most `MAX_TERMINALS` avatars are remembered: the ones offered longest ago go first, never one with a line in flight. */
  const trim = () => {
    if (terminals.size <= MAX_TERMINALS) return
    for (const [tid, entries] of terminals) {
      if (terminals.size <= MAX_TERMINALS) break
      let busy = false
      for (const entry of entries.values()) busy ||= inFlight(entry)
      if (busy) continue
      terminals.delete(tid)
      offered.delete(tid)
    }
  }

  const drop = (terminalId: string) => {
    const entries = terminals.get(terminalId)
    terminals.delete(terminalId)
    offered.delete(terminalId)
    if (entries) silenceAll([...entries.values()].filter(inFlight))
    speaker.drop(terminalId)
  }

  const isAgentLine = (line: unknown): line is Line => {
    const l = line as Line | null
    return typeof l === 'object' && l !== null && l.from === 'agent' && typeof l.id === 'string' && typeof l.text === 'string'
  }
  const outcomeOf = (line: Line): Outcome | undefined => (typeof line.outcome === 'string' ? OUTCOMES.get(line.outcome) : undefined)
  const voiceOf = (avatar: AvatarInfo): Voice => (typeof avatar.voice === 'object' && avatar.voice !== null ? avatar.voice : {})
  const langOf = (line: Line, voice: Voice): string => (typeof line.lang === 'string' ? line.lang : typeof voice.lang === 'string' ? voice.lang : 'en-US')
  const isAsk = (avatar: AvatarInfo, line: Line): boolean => (typeof avatar.ask?.lineId === 'string' && avatar.ask.lineId === line.id) || typeof line.askId === 'string'

  /** What this tab knows of a line it has not dealt with yet. */
  const entryFor = (avatar: AvatarInfo, line: Line): Entry => {
    const voice = voiceOf(avatar)
    return {
      key: lineKey(avatar.terminalId, line),
      terminalId: avatar.terminalId,
      lineId: line.id,
      text: line.text,
      lang: langOf(line, voice),
      voice,
      ask: isAsk(avatar, line),
      phase: 'waiting',
      timer: null,
      attempt: 0,
      leased: false,
      leasedAt: 0,
      leaseMs: 0,
      renewFailed: false,
      outcome: null,
      mode: null,
      presented: false,
      quiet: false,
      blockedStop: false,
      requeue: false,
      byUser: false,
      voiceName: null,
      retried: false,
    }
  }

  const replay = (avatar: AvatarInfo, line: Line): boolean => {
    if (disposed || typeof avatar !== 'object' || avatar === null || typeof avatar.terminalId !== 'string' || avatar.on === false || !isAgentLine(line)) return false
    const tid = avatar.terminalId
    // Hidden means silent, and an avatar whose session needs the user is quiet, whoever asks.
    if (!shows(tid) || blocked(tid)) return false
    let entries = terminals.get(tid)
    if (!entries) {
      entries = new Map<string, Entry>()
      terminals.set(tid, entries)
    }
    const key = lineKey(tid, line)
    let entry = entries.get(key)
    if (!entry) {
      entry = entryFor(avatar, line)
      entries.set(key, entry)
    }
    // What the record already says of the line is known, not news: it must not stop what the user asked for.
    const known = outcomeOf(line)
    if (known && rank(known) > rank(entry.outcome)) entry.outcome = known
    // It is being said here, or waits to be: nothing to add.
    if ((entry.phase === 'presenting' && entry.voiceName !== null) || (entry.phase === 'queued' && entry.mode === 'voice')) return true
    const voice = voiceOf(avatar)
    entry.text = line.text
    entry.lang = langOf(line, voice)
    entry.voice = voice
    entry.ask = isAsk(avatar, line)
    entry.byUser = true
    if (!inFlight(entry)) {
      // It was over here (a caption that left the screen, a line that was skipped): it starts anew.
      entry.quiet = false
      entry.blockedStop = false
      entry.voiceName = null
    }
    const onScreen = entry.phase === 'presenting'
    // A claim that is on its way finds the line with the speaker when it answers: it is late, and a yes is given back.
    present(entry, 'voice')
    // Its caption is on screen: the speaker starts it over aloud if it can, and leaves it be if it cannot.
    if (onScreen && entry.phase === 'queued') entry.phase = 'presenting'
    trim()
    return true
  }

  const offer = (avatar: AvatarInfo) => {
    if (disposed || typeof avatar !== 'object' || avatar === null || typeof avatar.terminalId !== 'string') return
    const tid = avatar.terminalId
    if (avatar.on === false) {
      drop(tid)
      return
    }
    // Offered last, trimmed last.
    const entries = terminals.get(tid) ?? new Map<string, Entry>()
    terminals.delete(tid)
    terminals.set(tid, entries)
    offered.set(tid, avatar)
    const lines: unknown = avatar.lines
    const listed = new Set<string>()
    if (Array.isArray(lines)) {
      for (const line of lines as Line[]) {
        if (!isAgentLine(line)) continue
        const key = lineKey(tid, line)
        listed.add(key)
        const known = entries.get(key)
        const outcome = outcomeOf(line)
        if (outcome) {
          if (known) settle(known, outcome)
          continue
        }
        if (known || !shows(tid)) continue
        const entry = entryFor(avatar, line)
        entries.set(key, entry)
        look(entry)
      }
    }
    // A line the record no longer lists cannot be offered again: nothing to remember once it is over.
    for (const [key, entry] of entries) if (!listed.has(key) && !inFlight(entry)) entries.delete(key)
    trim()
  }

  const setState = (s: ArbiterState) => {
    if (disposed || typeof s !== 'object' || s === null) return
    current = s
    for (const entries of terminals.values()) {
      for (const entry of entries.values()) {
        if (!inFlight(entry)) continue
        const tid = entry.terminalId
        if (entry.phase === 'waiting' || entry.phase === 'claiming') {
          // It was going to ask for the line: only while it could still say it.
          if (maySpeak(tid)) continue
          entry.attempt++
          look(entry)
          continue
        }
        if (!shows(tid)) {
          // Hidden now: it leaves the screen and falls silent, and the server hears nothing of it from here.
          silence(entry)
          continue
        }
        if (!blocked(tid)) continue
        // The session needs the user: its avatar stops talking, and what waits is shown silently.
        giveBack(entry)
        if (entry.phase === 'presenting') {
          if (entry.voiceName !== null) {
            entry.blockedStop = true
            speaker.stop(entry.key)
          }
        } else if (entry.mode === 'voice') {
          entry.requeue = true
          speaker.stop(entry.key)
          entry.requeue = false
          entry.presented = false
          caption(entry)
        }
      }
    }
    // Lines that were left alone while their avatar was hidden get their turn now.
    for (const avatar of Array.from(offered.values())) offer(avatar)
  }

  const stopAll = () => {
    const flying: Entry[] = []
    for (const entries of terminals.values()) for (const entry of entries.values()) if (inFlight(entry)) flying.push(entry)
    silenceAll(flying)
  }

  return {
    offer,
    settled(terminalId, line) {
      if (disposed || typeof line !== 'object' || line === null || typeof line.outcome !== 'string') return
      const outcome = OUTCOMES.get(line.outcome)
      const entry = terminals.get(terminalId)?.get(lineKey(terminalId, line))
      if (entry && outcome) settle(entry, outcome)
    },
    forget(terminalId) {
      if (!disposed) drop(terminalId)
    },
    replay,
    setState,
    release() {
      if (!disposed) stopAll()
    },
    dispose() {
      if (disposed) return
      stopAll()
      disposed = true
      unsubscribe()
      for (const id of timers) host.clearTimeout(id)
      timers.clear()
      terminals.clear()
      offered.clear()
      queued.clear()
    },
  }
}
