import { describe, expect, it } from 'vitest'
import { cmdText, needsAcknowledge, portText, relPath, riskCounts, sortedRisks, sourceText, stateLabel, stateTone } from './logic'
import type { Plan, Risk } from './types'

describe('devcontainer logic', () => {
  it('labels and tones states', () => {
    expect(stateLabel('running')).toBe('Running')
    expect(stateLabel('none')).toBe('Not created')
    expect(stateTone('error')).toBe('danger')
    expect(stateTone('building')).toBe('accent')
  })

  it('counts and sorts risks, dangers first', () => {
    const risks: Risk[] = [
      { level: 'info', item: 'image x', message: '' },
      { level: 'danger', item: '--privileged', message: '' },
      { level: 'warning', item: 'postCreateCommand', message: '' },
    ]
    expect(riskCounts(risks)).toEqual({ danger: 1, warning: 1, info: 1 })
    expect(sortedRisks(risks).map((r) => r.level)).toEqual(['danger', 'warning', 'info'])
    expect(needsAcknowledge({ risks } as Plan)).toBe(true)
    expect(needsAcknowledge({ risks: risks.filter((r) => r.level !== 'danger') } as Plan)).toBe(false)
  })

  it('shows sources and paths relative to the project', () => {
    expect(relPath('/home/u/app', '/home/u/app/.devcontainer/Dockerfile')).toBe('.devcontainer/Dockerfile')
    expect(relPath('/home/u/app', '/home/u/app')).toBe('.')
    expect(relPath('/home/u/app', '/etc/passwd')).toBe('/etc/passwd')
    expect(relPath('/home/u/app', '/home/u/app2/x')).toBe('/home/u/app2/x')
    expect(sourceText({ kind: 'image', image: 'debian:bookworm-slim' })).toBe('debian:bookworm-slim')
    expect(
      sourceText(
        { kind: 'dockerfile', dockerfile: '/r/.devcontainer/Dockerfile', context: '/r', args: {}, cacheFrom: [], options: [], target: 'dev' },
        '/r',
      ),
    ).toBe('.devcontainer/Dockerfile (context ., target dev)')
    expect(sourceText({ kind: 'compose', files: ['/r/a.yml'], service: 'app', runServices: [] }, '/r')).toBe('a.yml → service app')
  })

  it('renders commands and ports', () => {
    expect(cmdText({ kind: 'shell', value: 'npm ci && npm test' })).toBe('npm ci && npm test')
    expect(cmdText({ kind: 'exec', value: ['sh', '-c', "echo 'x'"] })).toBe(`sh -c 'echo '\\''x'\\'''`)
    expect(portText({ port: 8000, label: 'web', url: '', via: 'published', hostPort: 32768 })).toBe('8000 (web) → 127.0.0.1:32768')
    expect(portText({ port: 5432, label: null, url: '', via: 'container-ip', hostPort: null })).toBe('5432 → container address')
  })
})
