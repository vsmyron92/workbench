import { describe, expect, it } from 'vitest'
import { scanRequests } from './scan'

describe('scanRequests', () => {
  it('finds one request line per block, after comments and variables', () => {
    const text = ['@host = http://localhost', '', '### List', '# @name list', 'GET {{host}}/things', 'Accept: */*', '', '###', 'POST {{host}}/things', '', '{"a": 1}', '', '###', 'https://example.com/health', ''].join('\n')
    expect(scanRequests(text)).toEqual([
      { line: 5, method: 'GET' },
      { line: 9, method: 'POST' },
      { line: 14, method: 'GET' },
    ])
  })
  it('does not take a body line for a request', () => {
    expect(scanRequests('POST http://x\n\nGET is not a request here\n')).toEqual([{ line: 1, method: 'POST' }])
  })
})
