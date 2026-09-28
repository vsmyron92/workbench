// startRun's prompts: the server's live answers decide, not a stale cached list.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const post = vi.fn()
const confirmDialog = vi.fn()
const toastError = vi.fn()

vi.mock('@/api/client', () => {
  class ApiError extends Error {
    status: number
    code: string
    constructor(status: number, code: string, message: string) {
      super(message)
      this.status = status
      this.code = code
    }
  }
  return { ApiError, api: { post: (...a: unknown[]) => post(...a), get: vi.fn() } }
})
vi.mock('@/shell/actions', () => ({
  confirmDialog: (...a: unknown[]) => confirmDialog(...a),
  openPanel: vi.fn(),
  toast: vi.fn(),
  toastError: (...a: unknown[]) => toastError(...a),
}))

const { ApiError } = await import('@/api/client')
const { setAppsQueryClient, startRun } = await import('./api')
import type { QueryClient } from '@tanstack/react-query'
import type { RunView } from './types'

function cache(runs: Partial<RunView>[]) {
  const qc = { getQueryData: () => runs, invalidateQueries: vi.fn() } as unknown as QueryClient
  setAppsQueryClient(qc)
}

const web = (p: Partial<RunView> = {}): Partial<RunView> => ({
  name: 'web',
  state: 'stopped',
  portInUse: false,
  problems: [],
  config: { kind: 'server', command: 'npm run dev', cwd: 'web', port: 7824, freePort: false, dependsOn: [], env: {}, hasStop: false, hasStatus: false },
  ...p,
})

beforeEach(() => {
  post.mockReset()
  confirmDialog.mockReset()
  toastError.mockReset()
})

describe('startRun', () => {
  it('does not ask to free a port because of a stale cached portInUse', async () => {
    cache([web({ portInUse: true })])
    post.mockResolvedValueOnce({})
    expect(await startRun('p', 'web')).toBe(true)
    expect(confirmDialog).not.toHaveBeenCalled()
    expect(post).toHaveBeenCalledWith('/api/projects/p/runs/web/start', { freePort: false, confirmed: false })
  })

  it('asks when the server reports the port busy, then retries freeing it', async () => {
    cache([web()])
    post.mockRejectedValueOnce(new ApiError(409, 'port_in_use', 'port 7824 is in use by another process')).mockResolvedValueOnce({})
    confirmDialog.mockResolvedValueOnce(true)
    expect(await startRun('p', 'web')).toBe(true)
    expect(confirmDialog).toHaveBeenCalledTimes(1)
    expect(post).toHaveBeenLastCalledWith('/api/projects/p/runs/web/start', { freePort: true, confirmed: false })
  })

  it('does nothing more when the user keeps the port', async () => {
    cache([web()])
    post.mockRejectedValueOnce(new ApiError(409, 'port_in_use', 'busy'))
    confirmDialog.mockResolvedValueOnce(false)
    expect(await startRun('p', 'web')).toBe(false)
    expect(post).toHaveBeenCalledTimes(1)
  })

  it('confirms runs that may deploy before anything is sent', async () => {
    cache([web({ name: 'deploy (web)', needsConfirm: true })])
    confirmDialog.mockResolvedValueOnce(false)
    expect(await startRun('p', 'deploy (web)')).toBe(false)
    expect(post).not.toHaveBeenCalled()
    confirmDialog.mockResolvedValueOnce(true)
    post.mockResolvedValueOnce({})
    expect(await startRun('p', 'deploy (web)')).toBe(true)
    expect(post).toHaveBeenCalledWith('/api/projects/p/runs/deploy%20(web)/start', { freePort: false, confirmed: true })
  })

  it('confirms when the server says a dependency needs it', async () => {
    cache([web()])
    post.mockRejectedValueOnce(new ApiError(428, 'confirmation_required', 'deploy-prod may deploy')).mockResolvedValueOnce({})
    confirmDialog.mockResolvedValueOnce(true)
    expect(await startRun('p', 'web', true)).toBe(true)
    expect(post).toHaveBeenLastCalledWith('/api/projects/p/runs/web/restart', { freePort: false, confirmed: true })
  })

  it('reports other errors once', async () => {
    cache([web()])
    post.mockRejectedValueOnce(new ApiError(400, 'bad_request', 'no'))
    expect(await startRun('p', 'web')).toBe(false)
    expect(toastError).toHaveBeenCalledTimes(1)
  })
})
