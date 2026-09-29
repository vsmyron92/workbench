import { useState } from 'react'
import { Bell, BookOpen, Code, Columns2, Keyboard, Moon, Sun } from 'lucide-react'
import { FEATURES, useUnsupported } from '@/api/health'
import { useUi } from '@/state/store'
import { Button, Checkbox, ErrorBox, Input, Loading, TimeAgo } from '@/ui'
import { useSettings } from '../api'
import { CodeLine, Group, Page, Row, Segmented } from '../common'

function clampSize(v: string, fallback: number) {
  const n = Math.round(Number(v))
  return Number.isFinite(n) ? Math.min(28, Math.max(9, n)) : fallback
}

/** Per-device browser notifications (used on phones and other remote devices). */
export function BrowserNotificationsRow() {
  const enabled = useUi((s) => s.prefs.notifications)
  const setPrefs = useUi((s) => s.setPrefs)
  const supported = typeof Notification !== 'undefined'
  const [perm, setPerm] = useState<NotificationPermission | 'unsupported'>(supported ? Notification.permission : 'unsupported')
  const secure = window.isSecureContext
  const noDesktop = useUnsupported(FEATURES.desktopNotifications)
  return (
    <Row
      label="Browser notifications"
      hint={
        noDesktop
          ? 'While Workbench is in the background, on this computer too: its server shows no desktop notifications.'
          : 'On phones and other computers, while Workbench is in the background. The computer Workbench runs on gets desktop notifications instead.'
      }
    >
      <Checkbox checked={enabled} onChange={(v) => setPrefs({ notifications: v })}>
        Notify on this device
      </Checkbox>
      {perm === 'default' && secure && (
        <Button size="small" icon={Bell} onClick={() => void Notification.requestPermission().then(setPerm)}>
          Allow in this browser
        </Button>
      )}
      {perm === 'granted' && <span className="wb-small wb-success">Allowed</span>}
      {perm === 'denied' && <span className="wb-small wb-warning">Blocked in browser settings</span>}
      {(perm === 'unsupported' || !secure) && <span className="wb-small wb-muted">Needs HTTPS (or localhost) and a supporting browser</span>}
    </Row>
  )
}

export function GeneralSection() {
  const prefs = useUi((s) => s.prefs)
  const setPrefs = useUi((s) => s.setPrefs)
  const settings = useSettings()
  const s = settings.data
  return (
    <Page title="General" description="Appearance, editor and notification preferences are stored in this browser; everything else lives in config.toml on the server.">
      <Group title="Appearance">
        <Row label="Theme">
          <Segmented
            value={prefs.theme}
            onChange={(theme) => setPrefs({ theme })}
            options={[
              { value: 'dark', label: 'Dark', icon: Moon },
              { value: 'light', label: 'Light', icon: Sun },
            ]}
          />
        </Row>
        <Row label="Open Markdown files as" hint="Switch any open file with the Read, Split and Edit buttons in its toolbar.">
          <Segmented
            value={prefs.markdownMode ?? 'read'}
            onChange={(markdownMode) => setPrefs({ markdownMode })}
            options={[
              { value: 'read', label: 'Page', icon: BookOpen },
              { value: 'split', label: 'Split', icon: Columns2 },
              { value: 'edit', label: 'Source', icon: Code },
            ]}
          />
        </Row>
        <Row label="Editor keymap" hint="CLion's editing keys (Ctrl+D duplicate, Ctrl+Y delete line, Alt+J next occurrence, Ctrl+Shift+Up/Down move, Shift+Enter new line, Ctrl+Q docs…) or Monaco's own VS Code keys. Navigation and code intelligence keys are CLion's either way.">
          <Segmented
            value={prefs.editorKeymap ?? 'clion'}
            onChange={(editorKeymap) => setPrefs({ editorKeymap })}
            options={[
              { value: 'clion', label: 'CLion', icon: Keyboard },
              { value: 'vscode', label: 'VS Code', icon: Code },
            ]}
          />
        </Row>
        <Row label="Editor font size" hint="Monaco editors and diffs.">
          <Input
            type="number"
            min={9}
            max={28}
            value={prefs.editorFontSize}
            onChange={(e) => setPrefs({ editorFontSize: clampSize(e.target.value, prefs.editorFontSize) })}
            style={{ width: 90, flex: 'none', minWidth: 0 }}
          />
          <span className="wb-small wb-muted">px</span>
        </Row>
        <Row label="Terminal font size" hint="Agent sessions, shells and run output.">
          <Input
            type="number"
            min={9}
            max={28}
            value={prefs.terminalFontSize}
            onChange={(e) => setPrefs({ terminalFontSize: clampSize(e.target.value, prefs.terminalFontSize) })}
            style={{ width: 90, flex: 'none', minWidth: 0 }}
          />
          <span className="wb-small wb-muted">px</span>
        </Row>
      </Group>

      <Group title="Notifications">
        <BrowserNotificationsRow />
      </Group>

      <Group title="About this Workbench">
        {settings.isLoading ? (
          <Loading />
        ) : settings.error ? (
          <ErrorBox error={settings.error} onRetry={() => void settings.refetch()} />
        ) : s ? (
          <>
            <Row label="Version">
              <span className="mono">{s.version}</span>
              <span className="wb-small wb-muted">
                started <TimeAgo time={s.startedAt} />
              </span>
            </Row>
            <Row label="Listening on">
              <span className="mono">{s.bind}</span>
              {s.tlsActive && <span className="wb-small wb-success">HTTPS</span>}
            </Row>
            <Row label="Configuration">
              <span className="mono wb-small">{s.paths.configFile}</span>
            </Row>
            <Row label="Project overlays">
              <span className="mono wb-small">{s.paths.projectsDir}</span>
            </Row>
            <Row label="Data directory" hint="Token, device sessions, terminal state (all mode 600).">
              <span className="mono wb-small">{s.paths.dataDir}</span>
            </Row>
            <Row label="MCP endpoint" hint="Hosted Claude sessions reach Workbench's tools here.">
              <div style={{ flex: 1, maxWidth: 460 }}>
                <CodeLine text={s.mcpEndpoint} />
              </div>
            </Row>
          </>
        ) : null}
      </Group>
    </Page>
  )
}
