// The 'settings' panel: section navigation on the left, one section at a time.

import { useEffect, type ComponentType } from 'react'
import { AlertTriangle, Bell, Bot, Cable, CircleArrowUp, FileCode, FolderGit2, KeyRound, MonitorSmartphone, Palette, Plug } from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { Badge } from '@/ui'
import { useSecrets, useSettings, useUpdate } from './api'
import { restartText } from './lib'
import { AgentsSection } from './sections/Agents'
import { GeneralSection } from './sections/General'
import { IntegrationsSection } from './sections/Integrations'
import { McpSection } from './sections/Mcp'
import { NotificationsSection } from './sections/Notifications'
import { ProjectsSection } from './sections/Projects'
import { RawConfigSection } from './sections/RawConfig'
import { RemoteSection } from './sections/Remote'
import { SecretsSection } from './sections/Secrets'
import { UpdatesSection } from './sections/Updates'
import './platform.css'

export const SECTIONS: { id: string; label: string; icon: ComponentType<{ size?: number }> }[] = [
  { id: 'general', label: 'General', icon: Palette },
  { id: 'projects', label: 'Projects', icon: FolderGit2 },
  { id: 'agents', label: 'Agents', icon: Bot },
  { id: 'integrations', label: 'Integrations', icon: Plug },
  { id: 'secrets', label: 'Secrets', icon: KeyRound },
  { id: 'remote', label: 'Remote access', icon: MonitorSmartphone },
  { id: 'mcp', label: 'MCP', icon: Cable },
  { id: 'notifications', label: 'Notifications', icon: Bell },
  { id: 'updates', label: 'Updates', icon: CircleArrowUp },
  { id: 'raw', label: 'Raw config', icon: FileCode },
]

/** A warning count next to "Secrets" (unresolved or readable by others). */
function SecretsBadge() {
  const { data } = useSecrets()
  const n = (data?.secrets.filter((s) => !s.resolved || s.fixable).length ?? 0) + (data?.missing.length ?? 0)
  return n ? <Badge tone="warning">{n}</Badge> : null
}

/** The version next to "Updates" while a newer release waits. */
function UpdateBadge() {
  const { data } = useUpdate()
  return data?.available && data.latest ? <Badge tone="accent">{data.latest.version}</Badge> : null
}

export function SettingsPanel({ params, setParams, setTitle }: PanelProps<{ section?: string }>) {
  const section = SECTIONS.some((s) => s.id === params.section) ? params.section! : 'general'
  const settings = useSettings()
  const restart = settings.data?.restartRequired ?? []
  useEffect(() => setTitle('Settings'), [setTitle])
  const go = (id: string) => setParams({ ...params, section: id })

  let body
  switch (section) {
    case 'projects':
      body = <ProjectsSection />
      break
    case 'agents':
      body = <AgentsSection onGoto={go} />
      break
    case 'integrations':
      body = <IntegrationsSection onGoto={go} />
      break
    case 'secrets':
      body = <SecretsSection />
      break
    case 'remote':
      body = <RemoteSection />
      break
    case 'mcp':
      body = <McpSection />
      break
    case 'notifications':
      body = <NotificationsSection />
      break
    case 'updates':
      body = <UpdatesSection />
      break
    case 'raw':
      body = <RawConfigSection />
      break
    default:
      body = <GeneralSection />
  }

  return (
    <div className="wb-set">
      <nav className="wb-set-nav" aria-label="Settings sections">
        {SECTIONS.map((s) => (
          <button key={s.id} className={s.id === section ? 'wb-set-nav-item active' : 'wb-set-nav-item'} onClick={() => go(s.id)}>
            <s.icon size={15} />
            <span>{s.label}</span>
            {s.id === 'secrets' && <SecretsBadge />}
            {s.id === 'updates' && <UpdateBadge />}
          </button>
        ))}
      </nav>
      <div className="wb-set-main">
        {restart.length > 0 && (
          <div className="wb-set-banner">
            <AlertTriangle size={14} />
            Restart Workbench to apply the new {restartText(restart)}.
          </div>
        )}
        {section === 'raw' ? body : <div className="wb-set-scroll">{body}</div>}
      </div>
    </div>
  )
}
