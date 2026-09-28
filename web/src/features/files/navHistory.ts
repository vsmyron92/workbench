// Navigation history (CLion's Navigate Back / Forward, Recent Locations): where the
// caret has been in the editors, one entry per place. Small moves update the current
// entry; switching files or jumping more than NEAR lines starts a new one. Going back
// or forward does not add entries: its arrival updates the entry it went to.

export interface NavLocation {
  projectId: string | null
  path: string
  line: number
  column: number
}
export interface NavEntry extends NavLocation {
  at: number
}

/** Lines within which a move stays the same place. */
export const NEAR = 10
const MAX_ENTRIES = 100
/** How long an arrival (Back / Forward opening a file) is waited for. */
const ARRIVAL_MS = 3000

const sameFile = (a: NavLocation, b: NavLocation) => a.projectId === b.projectId && a.path === b.path

export class NavHistory {
  entries: NavEntry[] = []
  index = -1
  private arriving: { loc: NavLocation; until: number } | null = null

  /** The caret is at `loc` in the active editor (it moved, or that editor became active). */
  record(loc: NavLocation, now: number) {
    const cur = this.entries[this.index]
    if (this.arriving) {
      if (now > this.arriving.until) this.arriving = null
      else {
        // On the way to an entry: the editor reports where it was, then where it went.
        if (sameFile(loc, this.arriving.loc) && cur && sameFile(cur, loc)) {
          cur.line = loc.line
          cur.column = loc.column
          cur.at = now
          if (Math.abs(loc.line - this.arriving.loc.line) <= NEAR) this.arriving = null
        }
        return
      }
    }
    if (cur && sameFile(cur, loc) && Math.abs(cur.line - loc.line) <= NEAR) {
      cur.line = loc.line
      cur.column = loc.column
      cur.at = now
      return
    }
    this.entries = this.entries.slice(0, this.index + 1)
    this.entries.push({ ...loc, at: now })
    if (this.entries.length > MAX_ENTRIES) this.entries.shift()
    this.index = this.entries.length - 1
  }

  canBack() {
    return this.index > 0
  }
  canForward() {
    return this.index < this.entries.length - 1
  }

  back(now: number): NavEntry | null {
    if (!this.canBack()) return null
    return this.go(this.index - 1, now)
  }
  forward(now: number): NavEntry | null {
    if (!this.canForward()) return null
    return this.go(this.index + 1, now)
  }
  private go(i: number, now: number): NavEntry {
    this.index = i
    const e = this.entries[i]
    this.arriving = { loc: { ...e }, until: now + ARRIVAL_MS }
    return e
  }

  /** Distinct places, newest first (Recent Locations). */
  locations(): NavEntry[] {
    const out: NavEntry[] = []
    for (const e of [...this.entries].sort((a, b) => b.at - a.at)) {
      if (!out.some((o) => sameFile(o, e) && Math.abs(o.line - e.line) <= NEAR)) out.push(e)
    }
    return out
  }

  /** A file was deleted or renamed away: its places go. */
  forget(projectId: string | null, path: string) {
    const keep = this.entries.map((e) => !(e.projectId === projectId && e.path === path))
    const before = keep.slice(0, this.index + 1).filter((k) => !k).length
    this.entries = this.entries.filter((_, i) => keep[i])
    this.index = Math.min(this.entries.length - 1, Math.max(this.entries.length ? 0 : -1, this.index - before))
  }
}

export const navHistory = new NavHistory()
