import { useMemo, useState } from 'react'
import { BellRing, RotateCcw, Save, Smartphone } from 'lucide-react'
import { api } from '@/api/client'
import { FEATURES, useUnsupported } from '@/api/health'
import { toast, toastError } from '@/shell/actions'
import { Button, Checkbox, ErrorBox, Input, Loading } from '@/ui'
import { patchSettings, reportApply, useSettings } from '../api'
import { CodeLine, Group, Note, Page, Row, useDraft } from '../common'
import type { NotifyOutcome } from '../types'
import { BrowserNotificationsRow } from './General'
import { PushDevices, PushThisDevice } from './Push'

interface Form {
  desktop: boolean
  command: string
  subject: string
}

const normalize = (f: Form) => ({
  notify: { desktop: f.desktop, command: f.command.trim() || null },
  push: { subject: f.subject.trim() || null },
})

const DESKTOP_TEXT: Record<NotifyOutcome['desktop'], string> = {
  sent: 'desktop notification sent',
  disabled: 'desktop notifications are off',
  unavailable: 'notify-send is not installed',
  'rate-limited': 'rate-limited',
}

function subjectError(s: string): string | null {
  const v = s.trim()
  if (!v) return null
  if (/^mailto:[^\s@]+@[^\s@]+$/.test(v) || /^https:\/\/[^\s/@]+/.test(v)) return null
  return 'Use mailto:you@example.com or an https:// URL'
}

export function NotificationsSection() {
  const settings = useSettings()
  const n = settings.data?.config.notify
  const p = settings.data?.config.push
  const saved = useMemo(
    () => (settings.data ? { desktop: n?.desktop ?? true, command: n?.command ?? '', subject: p?.subject ?? '' } : undefined),
    [settings.data, n, p],
  )
  const { draft: form, setDraft: setForm, dirty, reset } = useDraft(saved, normalize)
  const [busy, setBusy] = useState(false)
  const [testing, setTesting] = useState(false)
  // Left out on some OSes (Windows): the server reports why instead of a missing notify-send.
  const desktopUnsupported = useUnsupported(FEATURES.desktopNotifications)

  if (settings.error) return <ErrorBox error={settings.error} onRetry={() => void settings.refetch()} />
  if (!form || !settings.data) return <Loading />

  const subjectProblem = subjectError(form.subject)
  const save = async () => {
    setBusy(true)
    try {
      reportApply(await patchSettings(normalize(form), settings.data?.hash), 'Notifications saved')
    } catch (e) {
      toastError(e, 'Could not save')
    } finally {
      setBusy(false)
    }
  }
  const test = async () => {
    setTesting(true)
    try {
      const r = await api.post<NotifyOutcome>('/api/platform/notify-test')
      const parts = [r.desktop === 'unavailable' && desktopUnsupported ? 'desktop notifications are not supported here' : (DESKTOP_TEXT[r.desktop] ?? r.desktop)]
      if (r.command === 'ran') parts.push('command started')
      toast(r.desktop === 'sent' || r.command === 'ran' ? 'success' : 'warning', `Test: ${parts.join(', ')}`)
    } catch (e) {
      toastError(e, 'Test failed')
    } finally {
      setTesting(false)
    }
  }

  return (
    <Page
      title="Notifications"
      description="Workbench tells you when an agent needs attention or finishes, an environment goes down or comes back, a deploy finishes, or a pipeline fails — on this computer's desktop and, with push, on your phone while Workbench is closed."
      actions={
        <>
          <Button icon={BellRing} loading={testing} disabled={dirty} title={dirty ? 'Save first' : undefined} onClick={() => void test()}>
            Test desktop
          </Button>
          <Button icon={RotateCcw} disabled={!dirty || busy} onClick={reset}>
            Revert
          </Button>
          <Button variant="primary" icon={Save} disabled={!dirty || !!subjectProblem} loading={busy} onClick={() => void save()}>
            Save
          </Button>
        </>
      }
    >
      <Group
        title="Push"
        description={
          <Note icon={Smartphone}>
            Phones need Workbench on <strong>HTTPS</strong> (for example <code>tailscale serve</code>, see Remote access) and a paired session. On iPhone and iPad, add
            Workbench to the Home Screen first (Safari › Share › Add to Home Screen) and turn push on in that app. Android and desktop Chrome, Edge and Firefox work in the
            browser and as an installed app.
          </Note>
        }
      >
        <PushThisDevice />
      </Group>

      <Group title="Devices with push" flush>
        <PushDevices />
      </Group>

      <Group title="On this computer">
        <Row
          label="Desktop notifications"
          hint={
            desktopUnsupported ??
            (settings.data.notifySend ? 'Shown with notify-send by the Workbench server.' : 'notify-send was not found on the server (install libnotify-bin).')
          }
        >
          <Checkbox checked={form.desktop} onChange={(desktop) => setForm({ ...form, desktop })}>
            Show desktop notifications
          </Checkbox>
        </Row>
        <Row
          top
          label="Command"
          hint="Run for every notification, e.g. to forward it elsewhere. The text is passed in environment variables, never inserted into the command."
        >
          <div style={{ flex: 1, display: 'flex', flexDirection: 'column', gap: 6, maxWidth: 560 }}>
            <Input
              className="mono"
              value={form.command}
              onChange={(e) => setForm({ ...form, command: e.target.value })}
              placeholder={'curl -s -d "$WORKBENCH_MESSAGE" -H "Title: $WORKBENCH_TITLE" ntfy.sh/my-topic'}
            />
            <div className="wb-xs wb-subtle">
              Runs with <code>sh -c</code>. Variables: <code>$WORKBENCH_TITLE</code>, <code>$WORKBENCH_MESSAGE</code>, <code>$WORKBENCH_LEVEL</code>{' '}
              (info, success, warning, error), <code>$WORKBENCH_EVENT</code>.
            </div>
          </div>
        </Row>
      </Group>

      <Group title="Without push">
        <BrowserNotificationsRow />
      </Group>

      <Group title="Push service">
        <Row
          label="VAPID contact"
          hint="Push services may use it to reach whoever runs this server. Default: the https public URL, else the address a device turned push on from."
        >
          <Input
            className="mono"
            value={form.subject}
            onChange={(e) => setForm({ ...form, subject: e.target.value })}
            placeholder="mailto:you@example.com"
            aria-invalid={!!subjectProblem}
          />
          {subjectProblem && <span className="wb-small wb-warning">{subjectProblem}</span>}
        </Row>
      </Group>

      <Group title="Rate limits" flush>
        <div className="wb-set-desc">
          At most one notification every 20 seconds per agent or environment, and 8 per minute overall; push follows the same limits and replaces an older push about
          the same agent. Agents can also notify you with the <code>workbench_notify</code> tool, for example:
        </div>
        <div style={{ marginTop: 8, maxWidth: 560 }}>
          <CodeLine text="When you are done, call workbench_notify with a one-line summary." />
        </div>
      </Group>
    </Page>
  )
}
