// Palette commands of the apps feature, built from the cached runs and environments.

import { ExternalLink, Eye, Play, RefreshCw, RotateCw, Rocket, ScrollText, Square } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import type { Command, CommandContext } from '@/shell/types'
import { cachedEnvs, cachedRuns, checkAllEnvs, openEnvLogs, openEnvPreview, openExternal, startRun, stopAllRuns, stopRun } from './api'
import { openDeployDialog } from './DeployDialog'
import { isActive } from './logic'

export function appsCommands(ctx: CommandContext): Command[] {
  const pid = ctx.projectId
  if (!pid) return []
  const runs = cachedRuns(pid)
  const envs = cachedEnvs(pid)
  const out: Command[] = []
  for (const r of runs) {
    const active = isActive(r.state)
    const suggested = r.config.group === 'suggested'
    out.push({
      id: `apps.run:${r.name}`,
      title: `Run ${r.name}`,
      group: suggested ? 'Run (suggested)' : 'Run',
      keywords: [r.config.command, r.config.kind],
      icon: Play,
      run: () => void startRun(pid, r.name),
      when: () => !active,
    })
    if (active) {
      out.push({ id: `apps.stop:${r.name}`, title: `Stop ${r.name}`, group: 'Run', icon: Square, run: () => void stopRun(pid, r.name) })
      out.push({ id: `apps.restart:${r.name}`, title: `Restart ${r.name}`, group: 'Run', icon: RotateCw, run: () => void startRun(pid, r.name, true) })
    }
  }
  if (runs.some((r) => isActive(r.state))) {
    out.push({ id: 'apps.stop-all', title: 'Stop all runs', group: 'Run', icon: Square, run: () => void stopAllRuns(pid) })
  }
  for (const e of envs) {
    out.push({ id: `apps.open:${e.name}`, title: `Open ${e.name}`, group: 'Environments', keywords: [e.url], icon: ExternalLink, run: () => openExternal(e.url) })
    out.push({ id: `apps.preview:${e.name}`, title: `Preview ${e.name}`, group: 'Environments', keywords: [e.url], icon: Eye, run: () => openEnvPreview(pid, e) })
    for (const l of e.config.logs) {
      out.push({ id: `apps.logs:${e.name}:${l.name}`, title: `Logs of ${e.name}: ${l.name}`, group: 'Environments', icon: ScrollText, run: () => void openEnvLogs(pid, e.name, l.name) })
    }
    if (e.config.deploy) {
      out.push({ id: `apps.deploy:${e.name}`, title: `Deploy to ${e.name}…`, group: 'Deploy', icon: Rocket, run: () => openDeployDialog(pid, e.name) })
    }
  }
  if (envs.length) {
    out.push({ id: 'apps.check-envs', title: 'Check environments now', group: 'Environments', icon: RefreshCw, run: () => void checkAllEnvs(pid) })
  }
  out.push({ id: 'apps.showRun', title: 'Show Run', group: 'Tool windows', shortcut: 'alt+4', keywords: ['output', 'run configurations'], icon: Play, run: () => showToolWindow('run', 'bottom') })
  return out
}
