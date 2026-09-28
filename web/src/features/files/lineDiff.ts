// Line diff for the editor's change markers (CLion's "line status" gutter): the
// buffer is compared live against the git base text, so markers follow typing.
// Myers' O((N+M)·D) algorithm on the region between the common prefix and suffix;
// when the edit distance is huge it degrades to one coarse block.

export interface ChangeBlock {
  kind: 'added' | 'modified' | 'deleted'
  /** 1-based inclusive lines in the current text; for `deleted`, `start === end` is
   *  the line after which lines were removed (0 = before the first line). */
  start: number
  end: number
  /** 0-based half-open range of the base lines this block replaces. */
  baseStart: number
  baseEnd: number
}

export function splitLines(text: string): string[] {
  return text.split(/\r\n|\r|\n/)
}

type Op = 0 | 1 | 2 // 0 equal, 1 delete (base), 2 insert (current)

function myers(a: Int32Array, b: Int32Array, maxD: number): Op[] | null {
  const n = a.length
  const m = b.length
  const max = n + m
  const off = max + 1
  const v = new Int32Array(2 * max + 3)
  const trace: Int32Array[] = []
  for (let d = 0; d <= max; d++) {
    if (d > maxD) return null
    trace.push(v.slice(off - d - 1, off + d + 2))
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1
      let y = x - k
      while (x < n && y < m && a[x] === b[y]) {
        x++
        y++
      }
      v[off + k] = x
      if (x >= n && y >= m) return backtrack(trace, n, m, d)
    }
  }
  return null
}

function backtrack(trace: Int32Array[], n: number, m: number, dEnd: number): Op[] {
  const ops: Op[] = []
  let x = n
  let y = m
  for (let d = dEnd; d >= 0; d--) {
    const t = trace[d]
    const get = (k: number) => t[k + d + 1]
    const k = x - y
    const prevK = k === -d || (k !== d && get(k - 1) < get(k + 1)) ? k + 1 : k - 1
    const prevX = get(prevK)
    const prevY = prevX - prevK
    while (x > prevX && y > prevY) {
      ops.push(0)
      x--
      y--
    }
    if (d > 0) {
      if (x === prevX) {
        ops.push(2)
        y--
      } else {
        ops.push(1)
        x--
      }
    }
    x = prevX
    y = prevY
  }
  return ops.reverse()
}

/** Change blocks of `cur` relative to `base`. */
export function diffLines(base: string[], cur: string[], maxD = 2000): ChangeBlock[] {
  let pre = 0
  while (pre < base.length && pre < cur.length && base[pre] === cur[pre]) pre++
  let suf = 0
  while (suf < base.length - pre && suf < cur.length - pre && base[base.length - 1 - suf] === cur[cur.length - 1 - suf]) suf++
  const aLines = base.slice(pre, base.length - suf)
  const bLines = cur.slice(pre, cur.length - suf)
  if (!aLines.length && !bLines.length) return []
  const ids = new Map<string, number>()
  const id = (s: string) => {
    let v = ids.get(s)
    if (v === undefined) ids.set(s, (v = ids.size))
    return v
  }
  const a = Int32Array.from(aLines, id)
  const b = Int32Array.from(bLines, id)
  const ops = myers(a, b, maxD)
  if (!ops) {
    // Too different to diff cheaply: one block over the whole middle.
    return [block(pre, pre + a.length, pre, pre + b.length)]
  }
  const out: ChangeBlock[] = []
  let ai = 0
  let bi = 0
  for (let i = 0; i < ops.length; ) {
    if (ops[i] === 0) {
      ai++
      bi++
      i++
      continue
    }
    const a0 = ai
    const b0 = bi
    while (i < ops.length && ops[i] !== 0) {
      if (ops[i] === 1) ai++
      else bi++
      i++
    }
    out.push(block(pre + a0, pre + ai, pre + b0, pre + bi))
  }
  return out
}

function block(baseStart: number, baseEnd: number, curStart: number, curEnd: number): ChangeBlock {
  const dels = baseEnd - baseStart
  const ins = curEnd - curStart
  if (ins === 0) return { kind: 'deleted', start: curStart, end: curStart, baseStart, baseEnd }
  return { kind: dels === 0 ? 'added' : 'modified', start: curStart + 1, end: curEnd, baseStart, baseEnd }
}

/** The current lines with `b` reverted to the base version ("Rollback lines"). */
export function rollbackBlock(cur: string[], base: string[], b: ChangeBlock): string[] {
  const restored = base.slice(b.baseStart, b.baseEnd)
  if (b.kind === 'deleted') return [...cur.slice(0, b.start), ...restored, ...cur.slice(b.start)]
  return [...cur.slice(0, b.start - 1), ...restored, ...cur.slice(b.end)]
}
