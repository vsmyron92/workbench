// The Services tool window's pure parts: the container tree, status tones, port
// links and Docker's time formats.

import type { DockerContainer, DockerImage, PortMap } from './api'

export type Tone = 'success' | 'warning' | 'danger' | 'accent' | 'muted'

/** A compose project with its containers, or a container of its own. */
export type TreeNode = { kind: 'compose'; project: string; containers: DockerContainer[]; projectId?: string; workingDir: string } | { kind: 'container'; c: DockerContainer }

export function isRunning(c: Pick<DockerContainer, 'state'>): boolean {
  return c.state === 'running' || c.state === 'restarting' || c.state === 'paused'
}

/** Exit code from Docker's status ("Exited (137) 2 hours ago"). */
export function exitCodeOf(status: string): number | null {
  const m = /^Exited \((-?\d+)\)/.exec(status)
  return m ? Number(m[1]) : null
}

export function containerTone(c: Pick<DockerContainer, 'state' | 'status'>): Tone {
  switch (c.state) {
    case 'running':
      if (/\(unhealthy\)/.test(c.status)) return 'danger'
      if (/\(health: starting\)/.test(c.status)) return 'warning'
      return 'success'
    case 'paused':
      return 'warning'
    case 'restarting':
    case 'removing':
      return 'accent'
    case 'dead':
      return 'danger'
    case 'exited': {
      const code = exitCodeOf(c.status)
      // 130 (Ctrl+C) and 143 (SIGTERM, `docker stop`) are clean stops.
      return code === null || code === 0 || code === 130 || code === 143 ? 'muted' : 'danger'
    }
    default:
      return 'muted'
  }
}

/** The label in a compose group: the service, with its replica number when there are several. */
export function serviceLabel(c: DockerContainer, siblings: DockerContainer[]): string {
  const svc = c.compose?.service || c.name
  const same = siblings.filter((s) => s.compose?.service === c.compose?.service).length
  return same > 1 && c.compose?.number ? `${svc} #${c.compose.number}` : svc
}

/**
 * Compose projects and single containers: the current project's first, then those
 * with something running, then by name.
 */
export function buildTree(list: DockerContainer[], currentProjectId: string | null): TreeNode[] {
  const groups = new Map<string, DockerContainer[]>()
  const nodes: TreeNode[] = []
  for (const c of list) {
    const p = c.compose?.project
    if (!p) {
      nodes.push({ kind: 'container', c })
      continue
    }
    const g = groups.get(p)
    if (g) g.push(c)
    else groups.set(p, [c])
  }
  for (const [project, containers] of groups) {
    containers.sort((a, b) => (a.compose?.service ?? '').localeCompare(b.compose?.service ?? '') || (a.compose?.number ?? 0) - (b.compose?.number ?? 0))
    const projectId = containers.find((c) => c.projectId)?.projectId
    nodes.push({ kind: 'compose', project, containers, projectId, workingDir: containers[0].compose?.workingDir ?? '' })
  }
  const pidOf = (n: TreeNode) => (n.kind === 'compose' ? n.projectId : n.c.projectId)
  const running = (n: TreeNode) => (n.kind === 'compose' ? n.containers.some(isRunning) : isRunning(n.c))
  const name = (n: TreeNode) => (n.kind === 'compose' ? n.project : n.c.name)
  const rank = (n: TreeNode) => (currentProjectId && pidOf(n) === currentProjectId ? 0 : 2) + (running(n) ? 0 : 1)
  return nodes.sort((a, b) => rank(a) - rank(b) || name(a).localeCompare(name(b)))
}

/** Keep the nodes of one Workbench project. */
export function filterTree(nodes: TreeNode[], projectId: string | null, text: string): TreeNode[] {
  const needle = text.trim().toLowerCase()
  const hit = (c: DockerContainer) => !needle || [c.name, c.image, c.compose?.service ?? '', c.compose?.project ?? ''].some((s) => s.toLowerCase().includes(needle))
  return nodes.flatMap((n): TreeNode[] => {
    if (n.kind === 'container') {
      if (projectId && n.c.projectId !== projectId) return []
      return hit(n.c) ? [n] : []
    }
    if (projectId && n.projectId !== projectId) return []
    const containers = n.project.toLowerCase().includes(needle) ? n.containers : n.containers.filter(hit)
    return containers.length ? [{ ...n, containers }] : []
  })
}

export function imageName(i: Pick<DockerImage, 'repository' | 'tag' | 'id'>): string {
  if (i.repository === '<none>') return `<none> ${shortId(i.id)}`
  return i.tag && i.tag !== '<none>' ? `${i.repository}:${i.tag}` : i.repository
}

/** What `docker rmi` takes for this row: the tag, or the id of a dangling image. */
export function imageRef(i: Pick<DockerImage, 'repository' | 'tag' | 'id'>): string {
  return i.repository === '<none>' || i.tag === '<none>' ? i.id : `${i.repository}:${i.tag}`
}

export function shortId(id: string): string {
  return id.replace(/^sha256:/, '').slice(0, 12)
}

const LOOPBACK = /^(127\.|localhost$|\[?::1\]?$)/

/**
 * Where a browser opens a published port: bound to every address, the host this page
 * came from; bound to loopback, only a browser on this computer can reach it.
 */
export function portUrl(p: PortMap, pageHost: string): string | null {
  if (!p.hostPort || p.proto !== 'tcp') return null
  const ip = p.hostIp
  let host: string
  if (!ip || ip === '0.0.0.0' || ip === '::') host = pageHost
  else if (LOOPBACK.test(ip)) {
    if (!LOOPBACK.test(pageHost)) return null
    host = 'localhost'
  } else host = ip.includes(':') ? `[${ip}]` : ip
  const scheme = p.port === 443 || p.port === 8443 ? 'https' : 'http'
  return `${scheme}://${host}:${p.hostPort}/`
}

export function portLabel(p: PortMap): string {
  if (!p.hostPort) return `${p.port}/${p.proto}`
  const ip = p.hostIp && p.hostIp !== '0.0.0.0' ? `${p.hostIp}:` : ''
  return `${ip}${p.hostPort} → ${p.port}${p.proto === 'tcp' ? '' : `/${p.proto}`}`
}

/**
 * Docker's times as epoch ms: RFC 3339 from inspect (nanoseconds allowed), and the
 * CLI's "2026-09-27 10:00:00 +0100 IST". `null` when absent or unreadable.
 */
export function dockerTime(s: string | null | undefined): number | null {
  if (!s) return null
  const cli = /^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}) ([+-])(\d{2})(\d{2})/.exec(s)
  const iso = cli ? `${cli[1]}T${cli[2]}${cli[3]}${cli[4]}:${cli[5]}` : s.replace(/(\.\d{3})\d+/, '$1')
  const t = Date.parse(iso)
  return Number.isNaN(t) ? null : t
}

/** The project-relative path of an absolute path inside `rootAbs`, else null. */
export function relativeTo(rootAbs: string, abs: string): string | null {
  const root = rootAbs.replace(/\/+$/, '')
  if (abs === root) return ''
  return abs.startsWith(`${root}/`) ? abs.slice(root.length + 1) : null
}
