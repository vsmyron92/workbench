import { describe, expect, it } from 'vitest'
import { ghUrl } from './api'
import { mobileViewFor } from './mobile'

describe('GitHub URLs', () => {
  it('follow the repository of a scope id', () => {
    expect(ghUrl('shop', 'summary')).toBe('/api/projects/shop/github/summary')
    expect(ghUrl('shop::services/api', 'runs/9')).toBe('/api/projects/shop/github/runs/9?repo=services%2Fapi')
  })
})

describe('GitHub panels on a phone', () => {
  it('keep the repository scope of the panel', () => {
    expect(mobileViewFor('pr', { projectId: 'shop::web', number: 4 }, 'shop')).toEqual({ kind: 'pr', pid: 'shop::web', number: 4 })
    expect(mobileViewFor('gh.run', { projectId: 'shop', runId: 3 }, null)).toEqual({ kind: 'run', pid: 'shop', id: 3 })
    expect(mobileViewFor('pr', { number: 4 }, null)).toBeNull()
  })
})
