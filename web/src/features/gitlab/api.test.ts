import { describe, expect, it } from 'vitest'
import { glUrl } from './api'

describe('GitLab URLs', () => {
  it('follow the repository of a scope id', () => {
    expect(glUrl('shop', 'summary')).toBe('/api/projects/shop/gitlab/summary')
    expect(glUrl('shop::services/api', 'pipelines/12')).toBe('/api/projects/shop/gitlab/pipelines/12?repo=services%2Fapi')
    expect(glUrl('shop::web', 'jobs/7/trace')).toBe('/api/projects/shop/gitlab/jobs/7/trace?repo=web')
  })
})
