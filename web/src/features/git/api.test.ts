import { describe, expect, it } from 'vitest'
import { gitUrl, gk } from './api'
import { gitStatusKey } from '@/features/files/hooks'

describe('git URLs', () => {
  it('have no repo parameter for the default repository, so a single repository is unchanged', () => {
    expect(gitUrl('shop', 'status')).toBe('/api/projects/shop/git/status')
    expect(gitUrl('shop', 'commits/abc123')).toBe('/api/projects/shop/git/commits/abc123')
  })

  it('name the real project and the repository of a scope id', () => {
    expect(gitUrl('shop::services/api', 'status')).toBe('/api/projects/shop/git/status?repo=services%2Fapi')
    expect(gitUrl('shop::web', 'ops/a%2Fb/cancel')).toBe('/api/projects/shop/git/ops/a%2Fb/cancel?repo=web')
    expect(gitUrl('shop::web', 'log?limit=5')).toBe('/api/projects/shop/git/log?limit=5&repo=web')
  })

  it('key every view by scope, so repositories never share a cache entry', () => {
    expect(gk.status('shop')).toEqual(['git', 'shop', 'status'])
    expect(gk.status('shop::web')).toEqual(['git', 'shop::web', 'status'])
    expect(gk.repos('shop')).toEqual(['git', 'shop', 'repos'])
    // The files tree’s all-repositories status is a key of the real project, under the plain status.
    expect(gk.statusAll('shop')).toEqual(['git', 'shop', 'status', 'all'])
    expect(gitStatusKey('shop', true)).toEqual(gk.statusAll('shop'))
    expect(gitStatusKey('shop')).toEqual(gk.status('shop'))
  })
})
