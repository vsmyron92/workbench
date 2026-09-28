import { describe, expect, it } from 'vitest'
import { layoutGraph, segmentPath, type GraphCommit } from './graph'

const c = (sha: string, ...parents: string[]): GraphCommit => ({ sha, parents })

/** Every bottom segment must end in a valid lane; every parent must be reached. */
function checkInvariants(commits: GraphCommit[]) {
  const { rows } = layoutGraph(commits)
  rows.forEach((r, i) => {
    const bottoms = r.segments.filter((s) => s.kind === 'bottom' && s.fromCol === r.col)
    expect(bottoms.length).toBeGreaterThanOrEqual(commits[i].parents.length > 0 ? 1 : 0)
    for (const s of r.segments) {
      expect(s.toCol).toBeGreaterThanOrEqual(0)
      expect(s.fromCol).toBeGreaterThanOrEqual(0)
    }
  })
  return rows
}

describe('layoutGraph', () => {
  it('keeps a linear history in one lane', () => {
    const rows = checkInvariants([c('c', 'b'), c('b', 'a'), c('a')])
    expect(rows.map((r) => r.col)).toEqual([0, 0, 0])
    expect(layoutGraph([c('c', 'b'), c('b', 'a'), c('a')]).maxWidth).toBe(1)
  })

  it('opens a lane for a merge and closes it at the fork point', () => {
    // m merges f into x; both come from base.
    const commits = [c('m', 'x', 'f'), c('f', 'base'), c('x', 'base'), c('base')]
    const rows = checkInvariants(commits)
    expect(rows[0].col).toBe(0)
    expect(rows[1].col).toBe(1) // the merged branch
    expect(rows[2].col).toBe(0)
    expect(rows[3].col).toBe(0)
    // base: the second lane converges into lane 0
    expect(rows[3].segments.some((s) => s.kind === 'top' && s.fromCol === 1 && s.toCol === 0)).toBe(true)
    expect(layoutGraph(commits).maxWidth).toBe(2)
  })

  it('handles two branch tips with a shared parent and root commits', () => {
    const rows = checkInvariants([c('t1', 'p'), c('t2', 'p'), c('p'), c('orphan')])
    expect(rows[0].col).toBe(0)
    expect(rows[1].col).toBe(1)
    expect(rows[2].col).toBe(0)
    expect(rows[3].col).toBe(0) // lanes were freed
  })

  it('gives lanes stable colours', () => {
    const { rows } = layoutGraph([c('m', 'x', 'f'), c('f', 'base'), c('x', 'base'), c('base')])
    expect(rows[1].color).not.toBe(rows[0].color)
    expect(rows[2].color).toBe(rows[0].color)
  })

  it('is fast on long histories', () => {
    const commits: GraphCommit[] = []
    for (let i = 0; i < 5000; i++) {
      const parents = i < 4999 ? [`c${i + 1}`] : []
      if (i % 50 === 0 && i + 5 < 5000) parents.push(`c${i + 5}`)
      commits.push(c(`c${i}`, ...parents))
    }
    const t = performance.now()
    const { rows } = layoutGraph(commits)
    expect(performance.now() - t).toBeLessThan(500)
    expect(rows.length).toBe(5000)
  })
})

describe('segmentPath', () => {
  it('draws straight lines and curves', () => {
    expect(segmentPath({ fromCol: 0, toCol: 0, kind: 'top', color: 0 }, 10, 20)).toBe('M5 0V10')
    expect(segmentPath({ fromCol: 0, toCol: 1, kind: 'bottom', color: 0 }, 10, 20)).toBe('M5 10C5 15 15 15 15 20')
  })
})
