// Dev container entries follow the server's health report: where its OS leaves dev
// containers out (Windows) only the Services tool window remains.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it } from 'vitest'
import { setHealth, type Health } from '@/api/health'
import { qk } from '@/api/queries'
import type { ProjectSummary } from '@/api/types'
import { devcontainerCommands } from './commands'
import { DevcontainerChip, DevcontainerStatus } from './widgets'

const base = { ok: true, service: 'workbench', version: '0.2.0', startedAt: 1, experimental: {} }
const LINUX: Health = { ...base, os: 'linux', unsupported: {} }
const WINDOWS: Health = { ...base, os: 'windows', unsupported: { devcontainer: 'dev containers are not supported on Windows yet' } }

type Dc = ProjectSummary['devcontainer']
const project = (devcontainer: Dc): ProjectSummary => ({
  id: 'shop',
  name: 'shop',
  root: '~/shop',
  rootAbs: '/home/u/shop',
  tags: [],
  docs: [],
  branch: 'main',
  gitlab: null,
  github: null,
  hasConfluence: false,
  hasJira: false,
  runs: 0,
  envs: [],
  warnings: [],
  devcontainer,
})
const RUNNING: Dc = { configs: ['.devcontainer/devcontainer.json'], state: 'running', inContainer: true }

/** The command ids the palette would show (what `featureCommands` keeps). */
function shown(p: ProjectSummary | null): string[] {
  const ctx = { projectId: p?.id ?? null, project: p }
  return devcontainerCommands(ctx)
    .filter((c) => !c.when || c.when(ctx))
    .map((c) => c.id)
}

function widgets(p: ProjectSummary): string {
  const qc = new QueryClient()
  qc.setQueryData(qk.projects, [p])
  const tree = createElement(QueryClientProvider, { client: qc }, createElement(DevcontainerChip, { projectId: p.id }), createElement(DevcontainerStatus, { projectId: p.id }))
  return renderToStaticMarkup(tree)
}

afterEach(() => setHealth(null))

describe('dev container entries', () => {
  it('are offered where the server supports dev containers', () => {
    setHealth(LINUX)
    expect(shown(project(null))).toEqual(['devcontainer.services', 'devcontainer.create'])
    expect(shown(project(RUNNING))).toEqual(['devcontainer.services', 'devcontainer.show', 'devcontainer.start', 'devcontainer.stop', 'devcontainer.rebuild', 'devcontainer.shell'])
    expect(widgets(project(RUNNING))).toContain('In container')
    // Before the health report arrives nothing is hidden (a Linux server reports nothing anyway).
    setHealth(null)
    expect(shown(project(null))).toContain('devcontainer.create')
  })

  it('leave only Services where the OS leaves dev containers out', () => {
    setHealth(WINDOWS)
    expect(shown(project(null))).toEqual(['devcontainer.services'])
    // Even with a summary (the server sends none there), nothing offers to start one.
    expect(shown(project(RUNNING))).toEqual(['devcontainer.services'])
    expect(shown(null)).toEqual(['devcontainer.services'])
    expect(widgets(project(RUNNING))).toBe('')
  })
})
