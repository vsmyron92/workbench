// The HTTP Client's selected environment (per project) and recent responses.

import { create } from 'zustand'
import type { HttpResult } from './api'

const ENV_KEY = (pid: string) => `wb.http.env.${pid}`
const MAX_RESULTS = 30

export function selectedEnv(pid: string): string | null {
  try {
    return localStorage.getItem(ENV_KEY(pid))
  } catch {
    return null
  }
}

export function selectEnv(pid: string, env: string | null) {
  try {
    if (env) localStorage.setItem(ENV_KEY(pid), env)
    else localStorage.removeItem(ENV_KEY(pid))
  } catch {
    /* unavailable */
  }
  useHttp.setState((s) => ({ envTick: s.envTick + 1 }))
}

export interface HttpRun {
  id: number
  projectId: string
  result?: HttpResult
  error?: string
  /** Sent, not answered yet. */
  pending?: { path: string; line: number; method: string; env: string | null; at: number }
}

export const useHttp = create<{ runs: HttpRun[]; selected: number | null; envTick: number }>()(() => ({ runs: [], selected: null, envTick: 0 }))

export function addRun(run: HttpRun) {
  useHttp.setState((s) => ({ runs: [run, ...s.runs.filter((r) => r.id !== run.id)].slice(0, MAX_RESULTS), selected: run.id }))
}
