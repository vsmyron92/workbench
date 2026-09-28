// One environment: status, latency sparkline, deployed version and actions.

import { useState } from 'react'
import { Ellipsis, ExternalLink, Eye, GitCommitHorizontal, RefreshCw, Rocket, ScrollText, SquareTerminal } from 'lucide-react'
import { Badge, IconButton, showMenu, showMenuAt, Spinner, StatusDot, TimeAgo, type MenuEntry } from '@/ui'
import { checkEnv, checkVersion, openEnvLogs, openEnvPreview, openExternal, openTerminal, runEnvCommand } from './api'
import { openDeployDialog } from './DeployDialog'
import { formatLatency, healthTone, shortSha, uptime } from './logic'
import { Sparkline } from './Sparkline'
import type { EnvView } from './types'

const KIND_TONE = { production: 'danger', staging: 'warning', preview: 'accent', development: undefined } as const

export function envMenu(pid: string, e: EnvView, onCheck?: () => void): MenuEntry[] {
  const items: MenuEntry[] = [
    { label: 'Open in browser', icon: ExternalLink, run: () => openExternal(e.url) },
    { label: 'Preview in Workbench', icon: Eye, run: () => openEnvPreview(pid, e) },
    'separator',
    { label: 'Check health now', icon: RefreshCw, disabled: !e.config.health, run: () => (onCheck ? onCheck() : void checkEnv(pid, e.name)) },
    { label: 'Check deployed version', icon: GitCommitHorizontal, disabled: !e.config.version, run: () => void checkVersion(pid, e.name) },
  ]
  if (e.config.logs.length) {
    items.push('separator')
    for (const l of e.config.logs) items.push({ label: `Logs: ${l.name}`, icon: ScrollText, run: () => void openEnvLogs(pid, e.name, l.name) })
  }
  if (e.config.commands.length) {
    items.push('separator')
    for (const c of e.config.commands) {
      items.push({ label: c.name + (c.confirm ? '…' : ''), icon: SquareTerminal, danger: c.confirm, run: () => void runEnvCommand(pid, e.name, c) })
    }
  }
  if (e.config.deploy) {
    items.push('separator', { label: `Deploy to ${e.name}…`, icon: Rocket, danger: e.kind === 'production', run: () => openDeployDialog(pid, e.name) })
  }
  return items
}

export function EnvCard({ pid, env: e }: { pid: string; env: EnvView }) {
  const [checking, setChecking] = useState(false)
  const h = e.health
  const tone = healthTone(h.status)
  const host = (() => {
    try {
      return new URL(e.url).host
    } catch {
      return e.url
    }
  })()
  const check = async () => {
    setChecking(true)
    await checkEnv(pid, e.name)
    setChecking(false)
  }
  const statusText =
    h.status === 'unknown'
      ? e.config.health
        ? h.error ?? 'not checked yet'
        : 'no health probe'
      : [h.httpStatus ? `HTTP ${h.httpStatus}` : null, formatLatency(h.latencyMs)].filter(Boolean).join(' · ')
  const version = e.version?.sha ?? null
  return (
    <div className={`wb-apps-env ${tone}`} onContextMenu={(ev) => showMenu(ev, envMenu(pid, e, check))}>
      <div className="wb-row head">
        <StatusDot tone={tone} pulse={checking} title={h.status} />
        <span className="name wb-ellipsis">{e.name}</span>
        {e.kind !== 'development' && !e.name.includes(e.kind) && <Badge tone={KIND_TONE[e.kind]}>{e.kind}</Badge>}
        {e.config.auth && (
          <span className="wb-xs wb-subtle" title={`Basic auth as ${e.config.auth.user} (secret ${e.config.auth.secret})`}>
            auth
          </span>
        )}
        <span className="wb-grow" />
        <IconButton size="small" icon={ExternalLink} label={`Open ${e.url}`} onClick={() => openExternal(e.url)} />
        <IconButton
          size="small"
          icon={Eye}
          label={e.preview.mode === 'proxy' ? `Preview (via local proxy: ${e.preview.reason ?? ''})` : 'Preview in Workbench'}
          onClick={() => openEnvPreview(pid, e)}
        />
        {e.config.logs.length > 0 && (
          <IconButton
            size="small"
            icon={ScrollText}
            label={e.config.logs.length === 1 ? `Logs (${e.config.logs[0].name})` : 'Logs…'}
            onClick={(ev) =>
              e.config.logs.length === 1
                ? void openEnvLogs(pid, e.name)
                : showMenuAt(
                    ev.currentTarget,
                    e.config.logs.map((l) => ({ label: l.name, icon: ScrollText, run: () => void openEnvLogs(pid, e.name, l.name) })),
                  )
            }
          />
        )}
        <IconButton size="small" icon={Ellipsis} label="More actions" onClick={(ev) => showMenuAt(ev.currentTarget, envMenu(pid, e, check))} />
      </div>
      <div className="wb-row meta">
        <span className="wb-ellipsis mono host" title={e.url}>
          {host}
        </span>
        <span className="wb-grow" />
        {checking ? <Spinner size={11} /> : <span className={`status ${tone}`}>{statusText}</span>}
      </div>
      {e.config.health && (
        <div className="wb-row spark">
          <div className="wb-grow">
            <Sparkline samples={h.history} width={300} height={20} fluid />
          </div>
          <span className="wb-xs wb-subtle uptime" title="Successful checks among the recorded ones (last 60)">
            {uptime(h.history)}
          </span>
        </div>
      )}
      {h.error && h.status !== 'unknown' && (
        <div className="wb-xs err wb-ellipsis" title={h.error}>
          {h.error}
        </div>
      )}
      <div className="wb-row foot">
        {(e.config.version || version) && (
          <button
            className="wb-apps-version"
            title={
              e.version?.error
                ? `Version check failed: ${e.version.error}`
                : e.version?.raw
                  ? `${e.version.raw}\n(${e.version.source}; click to re-check)`
                  : `Check the deployed version${e.config.version?.command && !e.config.version.http ? ` (runs a command on ${e.config.target ?? 'the host'})` : ''}`
            }
            disabled={!e.config.version}
            onClick={() => void checkVersion(pid, e.name)}
          >
            <GitCommitHorizontal size={12} />
            {version ? <span className="mono">{shortSha(version)}</span> : <span className="wb-muted">{e.version?.error ? 'version ?' : 'check version'}</span>}
          </button>
        )}
        {h.checkedAt && (
          <span className="wb-xs wb-subtle">
            <TimeAgo time={h.checkedAt} ms={15_000} />
          </span>
        )}
        <span className="wb-grow" />
        {e.deploying ? (
          <button className="wb-apps-deploying" onClick={() => openTerminal(e.deploying!, `Deploy → ${e.name}`)}>
            <Spinner size={11} /> deploying…
          </button>
        ) : (
          e.config.deploy && (
            <button className="wb-btn small" onClick={() => openDeployDialog(pid, e.name)} title={`Deploy to ${e.name}`}>
              <Rocket size={13} /> Deploy…
            </button>
          )
        )}
      </div>
    </div>
  )
}
