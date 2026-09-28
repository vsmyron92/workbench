// Pure helpers of the devcontainer feature (unit-tested).

import type { Cmd, DcState, DevConfig, Plan, PortView, Risk, RiskLevel, Source } from './types'

export type Tone = 'success' | 'accent' | 'warning' | 'danger' | 'muted'

export function stateLabel(s: DcState): string {
  switch (s) {
    case 'running':
      return 'Running'
    case 'stopped':
      return 'Stopped'
    case 'building':
      return 'Starting…'
    case 'error':
      return 'Failed'
    default:
      return 'Not created'
  }
}

export function stateTone(s: DcState): Tone {
  switch (s) {
    case 'running':
      return 'success'
    case 'building':
      return 'accent'
    case 'error':
      return 'danger'
    case 'stopped':
      return 'warning'
    default:
      return 'muted'
  }
}

export function riskCounts(risks: Risk[]): Record<RiskLevel, number> {
  const c: Record<RiskLevel, number> = { danger: 0, warning: 0, info: 0 }
  for (const r of risks) c[r.level]++
  return c
}

/** Risks sorted dangers first (the server sorts too; kept stable here). */
export function sortedRisks(risks: Risk[]): Risk[] {
  const order: Record<RiskLevel, number> = { danger: 0, warning: 1, info: 2 }
  return [...risks].sort((a, b) => order[a.level] - order[b.level])
}

/** A path shown relative to the project root when it is inside it. */
export function relPath(rootAbs: string | undefined, p: string): string {
  if (!rootAbs) return p
  const root = rootAbs.replace(/\/+$/, '')
  if (p === root) return '.'
  return p.startsWith(root + '/') ? p.slice(root.length + 1) : p
}

export function sourceText(s: Source, rootAbs?: string): string {
  switch (s.kind) {
    case 'image':
      return s.image
    case 'dockerfile':
      return `${relPath(rootAbs, s.dockerfile)} (context ${relPath(rootAbs, s.context)}${s.target ? `, target ${s.target}` : ''})`
    case 'compose':
      return `${s.files.map((f) => relPath(rootAbs, f)).join(', ')} → service ${s.service}`
    default:
      return 'nothing to start'
  }
}

export function sourceKindLabel(s: Source): string {
  return s.kind === 'image' ? 'Image' : s.kind === 'dockerfile' ? 'Dockerfile' : s.kind === 'compose' ? 'Compose' : 'Source'
}

export function cmdText(c: Cmd): string {
  return c.kind === 'shell' ? c.value : c.value.map((a) => (/^[\w./:=@%+-]+$/.test(a) ? a : `'${a.replace(/'/g, `'\\''`)}'`)).join(' ')
}

export function engineLabel(p: Plan | null): string {
  if (!p) return '—'
  return p.engine ? p.engineNote : `unavailable (${p.engineNote})`
}

/** Environment variable names (values of host variables are never shown). */
export function envNames(c: DevConfig): { container: string[]; remote: string[] } {
  return { container: Object.keys(c.containerEnv), remote: Object.keys(c.remoteEnv) }
}

export function portText(p: PortView): string {
  const where = p.via === 'published' ? `127.0.0.1:${p.hostPort ?? p.port}` : p.via === 'container-ip' ? 'container address' : 'host network'
  return `${p.port}${p.label ? ` (${p.label})` : ''} → ${where}`
}

/** What a Start of `plan` needs before the button is enabled. */
export function needsAcknowledge(plan: Plan): boolean {
  return plan.risks.some((r) => r.level === 'danger')
}
