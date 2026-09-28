import { describe, expect, it } from 'vitest'
import type { DockerContainer } from './api'
import { buildTree, containerTone, dockerTime, exitCodeOf, filterTree, imageName, imageRef, portLabel, portUrl, relativeTo, serviceLabel } from './logic'

const c = (over: Partial<DockerContainer>): DockerContainer => ({
  id: over.name ?? 'x',
  name: 'x',
  image: 'img',
  state: 'running',
  status: 'Up 2 hours',
  createdAt: '2026-09-27 10:00:00 +0100 IST',
  ports: [],
  ...over,
})
const svc = (project: string, service: string, over: Partial<DockerContainer> = {}) =>
  c({ name: `${project}-${service}-1`, compose: { project, service, workingDir: `/src/${project}`, configFiles: [], number: 1 }, ...over })

describe('services tree', () => {
  it('groups compose projects and puts the current project, then running ones, first', () => {
    const tree = buildTree(
      [
        c({ name: 'zeta', state: 'exited', status: 'Exited (0) 1 day ago' }),
        svc('shop', 'web'),
        svc('shop', 'db'),
        c({ name: 'alpha' }),
        svc('mine', 'api', { projectId: 'p1', state: 'exited', status: 'Exited (1) 1 hour ago' }),
      ],
      'p1',
    )
    expect(tree.map((n) => (n.kind === 'compose' ? `[${n.project}]` : n.c.name))).toEqual(['[mine]', 'alpha', '[shop]', 'zeta'])
    const shop = tree[2]
    expect(shop.kind === 'compose' && shop.containers.map((x) => x.compose?.service)).toEqual(['db', 'web'])
    expect(tree[0].kind === 'compose' && tree[0].projectId).toBe('p1')
  })

  it('filters by project and text', () => {
    const tree = buildTree([svc('shop', 'web'), svc('shop', 'db', { image: 'postgres:16' }), c({ name: 'alpha', projectId: 'p1' })], null)
    expect(filterTree(tree, 'p1', '').map((n) => n.kind)).toEqual(['container'])
    const f = filterTree(tree, null, 'postgres')
    expect(f).toHaveLength(1)
    expect(f[0].kind === 'compose' && f[0].containers.map((x) => x.compose?.service)).toEqual(['db'])
    // Matching the compose project keeps all of its containers.
    expect(filterTree(tree, null, 'SHOP')[0]).toMatchObject({ kind: 'compose', containers: [{}, {}] })
  })

  it('labels replicas', () => {
    const a = svc('shop', 'web', { compose: { project: 'shop', service: 'web', workingDir: '', configFiles: [], number: 2 } })
    expect(serviceLabel(a, [a, svc('shop', 'web')])).toBe('web #2')
    expect(serviceLabel(a, [a])).toBe('web')
  })
})

describe('status tones', () => {
  it('follows state, health and exit codes', () => {
    expect(containerTone({ state: 'running', status: 'Up 2 hours (healthy)' })).toBe('success')
    expect(containerTone({ state: 'running', status: 'Up 1 second (health: starting)' })).toBe('warning')
    expect(containerTone({ state: 'running', status: 'Up 5 minutes (unhealthy)' })).toBe('danger')
    expect(containerTone({ state: 'exited', status: 'Exited (0) 3 days ago' })).toBe('muted')
    expect(containerTone({ state: 'exited', status: 'Exited (143) 3 days ago' })).toBe('muted')
    expect(containerTone({ state: 'exited', status: 'Exited (1) 3 days ago' })).toBe('danger')
    expect(containerTone({ state: 'paused', status: 'Up 1 hour (Paused)' })).toBe('warning')
    expect(containerTone({ state: 'created', status: 'Created' })).toBe('muted')
    expect(exitCodeOf('Exited (137) 2 hours ago')).toBe(137)
    expect(exitCodeOf('Up 2 hours')).toBeNull()
  })
})

describe('ports', () => {
  it('links published ports the browser can reach', () => {
    expect(portUrl({ port: 80, proto: 'tcp', hostIp: '0.0.0.0', hostPort: 8080 }, 'box.tailnet.ts.net')).toBe('http://box.tailnet.ts.net:8080/')
    expect(portUrl({ port: 80, proto: 'tcp', hostPort: 8080 }, '127.0.0.1')).toBe('http://127.0.0.1:8080/')
    expect(portUrl({ port: 5173, proto: 'tcp', hostIp: '127.0.0.1', hostPort: 5173 }, '127.0.0.1')).toBe('http://localhost:5173/')
    expect(portUrl({ port: 5173, proto: 'tcp', hostIp: '127.0.0.1', hostPort: 5173 }, 'box.tailnet.ts.net')).toBeNull()
    expect(portUrl({ port: 443, proto: 'tcp', hostIp: '10.0.0.5', hostPort: 8443 }, 'localhost')).toBe('https://10.0.0.5:8443/')
    expect(portUrl({ port: 5432, proto: 'tcp' }, 'localhost')).toBeNull()
    expect(portUrl({ port: 53, proto: 'udp', hostPort: 53 }, 'localhost')).toBeNull()
    expect(portLabel({ port: 80, proto: 'tcp', hostIp: '127.0.0.1', hostPort: 8080 })).toBe('127.0.0.1:8080 → 80')
    expect(portLabel({ port: 80, proto: 'tcp', hostIp: '0.0.0.0', hostPort: 8080 })).toBe('8080 → 80')
    expect(portLabel({ port: 5432, proto: 'tcp' })).toBe('5432/tcp')
  })
})

describe('images', () => {
  it('names and references', () => {
    expect(imageName({ repository: 'nginx', tag: '1', id: 'sha256:abc' })).toBe('nginx:1')
    expect(imageName({ repository: '<none>', tag: '<none>', id: 'sha256:0123456789abcdef' })).toBe('<none> 0123456789ab')
    expect(imageRef({ repository: '<none>', tag: '<none>', id: 'sha256:0123' })).toBe('sha256:0123')
    expect(imageRef({ repository: 'reg.io/g/p', tag: 'v1', id: 'sha256:0123' })).toBe('reg.io/g/p:v1')
  })
})

describe('times and paths', () => {
  it('reads both of Docker time formats', () => {
    expect(dockerTime('2026-09-27 10:00:00 +0100 IST')).toBe(Date.parse('2026-09-27T09:00:00Z'))
    expect(dockerTime('2026-09-27T09:00:01.123456789Z')).toBe(Date.parse('2026-09-27T09:00:01.123Z'))
    expect(dockerTime('')).toBeNull()
    expect(dockerTime('garbage')).toBeNull()
  })
  it('relates absolute paths to a project root', () => {
    expect(relativeTo('/src/shop', '/src/shop/deploy/compose.yaml')).toBe('deploy/compose.yaml')
    expect(relativeTo('/src/shop/', '/src/shop')).toBe('')
    expect(relativeTo('/src/shop', '/src/shopping/x')).toBeNull()
  })
})
