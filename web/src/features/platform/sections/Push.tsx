// Web Push in Settings › Notifications and on the phone's More tab: this device's
// push (turn on/off, what to send, test) and every device that receives push.

import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { BellOff, BellRing, Laptop, Send, Smartphone, Trash2 } from 'lucide-react'
import { api } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Badge, Button, Checkbox, EmptyState, ErrorBox, Loading, TimeAgo } from '@/ui'
import { pk, usePushInfo, useRemote, useSettings } from '../api'
import { Note, Row } from '../common'
import { isLoopbackHost } from '../lib'
import { currentSupport, disablePush, enablePush, notificationPermission } from '../push'
import type { DeviceInfo, PushDelivery, PushInfo, PushSubscriptionInfo, PushTopics } from '../types'

const TOPICS: { key: keyof PushTopics; label: string; hint: string }[] = [
  { key: 'attention', label: 'An agent needs me', hint: 'Permission requests (with Allow and Deny), questions and errors' },
  { key: 'done', label: 'An agent finished', hint: 'The first line of its answer' },
  { key: 'env', label: 'Environments', hint: 'Down, and back up' },
  { key: 'deploy', label: 'Deploys', hint: 'Finished or failed' },
  { key: 'pipeline', label: 'CI', hint: 'Failed pipelines and workflow runs' },
  { key: 'notify', label: 'Agent messages', hint: 'What agents send with workbench_notify' },
]

function isPhone(name: string, ua = '') {
  return /Android|iOS/.test(name) || /Mobile|Android|iPhone|iPad/.test(ua)
}

function deliveryText(r: PushDelivery): string {
  switch (r.outcome) {
    case 'sent':
      return `Test sent to ${r.device}`
    case 'gone':
      return `${r.device}: the push service no longer knows this device; turn push on there again`
    default:
      return `${r.device}: ${r.error ?? r.outcome}`
  }
}

export async function sendTestPush(sub?: PushSubscriptionInfo) {
  try {
    const out = await api.post<{ results: PushDelivery[] }>('/api/push/test', sub ? { subscriptionId: sub.id } : {})
    const r = out.results[0]
    if (!r) toast('warning', 'Nothing was sent')
    else toast(r.outcome === 'sent' ? 'success' : 'warning', deliveryText(r), { timeout: r.outcome === 'sent' ? 4000 : 9000 })
  } catch (e) {
    toastError(e, 'Test failed')
  }
}

/** "Send test push" with its own busy state (the push service answers in a second or two). */
function TestButton({ sub, label, variant, size }: { sub: PushSubscriptionInfo; label: string; variant?: 'ghost'; size?: 'small' }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState(false)
  return (
    <Button
      size={size}
      variant={variant}
      icon={Send}
      loading={busy}
      onClick={() => {
        setBusy(true)
        void sendTestPush(sub).finally(() => {
          setBusy(false)
          // "Last push" shows what the push service answered.
          void qc.invalidateQueries({ queryKey: pk.push })
        })
      }}
    >
      {label}
    </Button>
  )
}

function usePatch() {
  const qc = useQueryClient()
  return async (sub: PushSubscriptionInfo, body: { topics?: PushTopics; quietWhenActive?: boolean }) => {
    // Show the change at once; the server's answer (and `push.changed`) confirms it.
    qc.setQueryData<PushInfo>(pk.push, (old) => old && { ...old, subscriptions: old.subscriptions.map((s) => (s.id === sub.id ? { ...s, ...body } : s)) })
    try {
      await api.patch(`/api/push/subscriptions/${encodeURIComponent(sub.id)}`, body)
    } catch (e) {
      toastError(e, 'Could not save')
    }
    await qc.invalidateQueries({ queryKey: pk.push })
  }
}

/** Why push cannot work here (HTTPS, iOS home screen, browser support), or null. */
function Unsupported() {
  const s = currentSupport()
  if (s.ok) return null
  if (s.reason === 'insecure') {
    return (
      <Note tone="warning">
        Push needs HTTPS. Open Workbench through an https address, for example with <code>tailscale serve</code>, a reverse proxy or <code>[server.tls]</code>{' '}
        (Settings › Remote access), and turn push on from there.
      </Note>
    )
  }
  return <Note tone="warning">{s.message}</Note>
}

/**
 * This page is on the computer Workbench runs on (a loopback address, as for in-tab
 * notifications), which shows the server's desktop notifications: push here would
 * show each one twice.
 */
function useHostHasDesktop(): boolean {
  const settings = useSettings()
  const s = settings.data
  return isLoopbackHost(location.hostname) && !!s && s.notifySend && s.config.notify?.desktop !== false
}

/** Turn push on or off for this browser. */
function OnOffButton({ info, mine, anyway }: { info: PushInfo; mine: PushSubscriptionInfo | undefined; anyway?: boolean }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState(false)
  const run = async (fn: () => Promise<unknown>, done: string, fail: string) => {
    setBusy(true)
    try {
      await fn()
      toast('success', done)
    } catch (e) {
      toastError(e, fail)
    } finally {
      setBusy(false)
      await qc.invalidateQueries({ queryKey: pk.push })
    }
  }
  if (mine) {
    return (
      <Button icon={BellOff} loading={busy} onClick={() => void run(() => disablePush(info), 'Push is off on this device', 'Could not turn push off')}>
        Turn off
      </Button>
    )
  }
  return (
    <Button
      variant={anyway ? undefined : 'primary'}
      icon={BellRing}
      loading={busy}
      onClick={() => void run(() => enablePush(info), 'Push is on for this device', 'Could not turn push on')}
    >
      {anyway ? 'Turn on anyway' : 'Turn on push'}
    </Button>
  )
}

/** Why the Workbench computer needs no push, or why it now shows everything twice. */
function HostDesktopNote({ on }: { on: boolean }) {
  return on ? (
    <Note tone="warning">
      Workbench runs on this computer and shows its desktop notifications here too, so each one appears twice. Turn push off here, or turn desktop notifications off
      (Settings › Notifications › On this computer).
    </Note>
  ) : (
    <Note>
      Workbench runs on this computer, which already shows its desktop notifications; push here would show each one twice. Turn it on anyway for Allow and Deny in the
      notification, and turn desktop notifications off (Settings › Notifications › On this computer).
    </Note>
  )
}

function TopicChecks({ sub }: { sub: PushSubscriptionInfo }) {
  const patch = usePatch()
  return (
    <div className="wb-push-topics">
      {TOPICS.map((t) => (
        <Checkbox key={t.key} checked={sub.topics[t.key]} onChange={(v) => void patch(sub, { topics: { ...sub.topics, [t.key]: v } })}>
          <span>
            {t.label}
            <span className="wb-push-topic-hint">{t.hint}</span>
          </span>
        </Checkbox>
      ))}
    </div>
  )
}

function LastDelivery({ sub }: { sub: PushSubscriptionInfo }) {
  if (sub.lastError && (!sub.lastOkAt || (sub.lastErrorAt ?? 0) > sub.lastOkAt)) {
    return (
      <span className="wb-warning wb-small wb-push-error" title={sub.lastError}>
        Failed <TimeAgo time={sub.lastErrorAt} />: {sub.lastError}
      </span>
    )
  }
  if (sub.lastOkAt) {
    return (
      <span className="wb-muted wb-small">
        Delivered <TimeAgo time={sub.lastOkAt} />
      </span>
    )
  }
  return <span className="wb-subtle wb-small">Nothing sent yet</span>
}

/** Settings › Notifications: this device. */
export function PushThisDevice() {
  const push = usePushInfo()
  const patch = usePatch()
  const hostDesktop = useHostHasDesktop()
  if (push.error) return <ErrorBox error={push.error} onRetry={() => void push.refetch()} />
  if (!push.data) return <Loading />
  const info = push.data
  const mine = info.subscriptions.find((s) => s.current)
  const support = currentSupport()
  const denied = support.ok && notificationPermission() === 'denied' && !mine
  return (
    <>
      <Row
        label="Push to this device"
        hint="Reaches this device while Workbench is closed or in the background, like an app: “Claude needs your permission” with Allow and Deny on the lock screen."
      >
        {!support.ok ? (
          <span className="wb-small wb-muted">Not available here</span>
        ) : denied ? (
          <span className="wb-small wb-warning">Notifications are blocked in this browser’s site settings</span>
        ) : (
          <>
            {mine && (
              <Badge tone="success" title={`Through ${mine.service}`}>
                On
              </Badge>
            )}
            {mine && <span className="wb-small wb-muted">via {mine.service}</span>}
            {!mine && hostDesktop && <span className="wb-small wb-muted">Desktop notifications cover this computer</span>}
            <span className="wb-grow" />
            {mine && <TestButton sub={mine} label="Send test push" />}
            <OnOffButton info={info} mine={mine} anyway={hostDesktop} />
          </>
        )}
      </Row>
      {(!support.ok || (hostDesktop && !denied)) && (
        <div className="wb-set-row">
          <div />
          <div className="control">{support.ok ? <HostDesktopNote on={!!mine} /> : <Unsupported />}</div>
        </div>
      )}
      {mine && (
        <>
          <Row top label="Send me" hint="Titles and one-line summaries only: no code, file contents or secrets.">
            <TopicChecks sub={mine} />
          </Row>
          <Row
            label="Other devices"
            hint="A device that shows Workbench never gets a push. Holding also skips pushes while another device was used in the last two minutes."
          >
            <Checkbox checked={mine.quietWhenActive} onChange={(v) => void patch(mine, { quietWhenActive: v })}>
              Hold pushes while I am using Workbench on another device
            </Checkbox>
          </Row>
          <Row label="Last push">
            <LastDelivery sub={mine} />
          </Row>
        </>
      )}
    </>
  )
}

/** Settings › Notifications: every device with push, named after its device session. */
export function PushDevices() {
  const qc = useQueryClient()
  const push = usePushInfo()
  const remote = useRemote()
  if (push.error) return <ErrorBox error={push.error} onRetry={() => void push.refetch()} />
  if (!push.data) return <Loading />
  const subs = push.data.subscriptions
  if (!subs.length) {
    return (
      <div className="wb-set-box">
        <EmptyState icon={BellRing} title="No device receives push yet">
          Turn push on above, or from the More tab on a paired phone.
        </EmptyState>
      </div>
    )
  }
  const devices = new Map<string, DeviceInfo>((remote.data?.devices ?? []).map((d) => [d.id, d]))
  const remove = async (s: PushSubscriptionInfo, name: string) => {
    const ok = await confirmDialog({
      title: `Stop push to “${name}”?`,
      message: s.current ? 'This device stops receiving push notifications.' : 'That device stops receiving push notifications until it turns push on again.',
      confirmLabel: 'Stop push',
      danger: true,
    })
    if (!ok) return
    try {
      if (s.current) await disablePush(push.data)
      else await api.del(`/api/push/subscriptions/${encodeURIComponent(s.id)}`)
      toast('success', `Push stopped for ${name}`)
    } catch (e) {
      toastError(e, 'Could not remove')
    }
    await qc.invalidateQueries({ queryKey: pk.push })
  }
  return (
    <div className="wb-set-box">
      <table className="wb-pf-table">
        <thead>
          <tr>
            <th>Device</th>
            <th>Service</th>
            <th>Receives</th>
            <th>Last push</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {subs.map((s) => {
            const d = devices.get(s.sessionId)
            const name = d?.name ?? s.device
            const Icon = isPhone(name, d?.userAgent) ? Smartphone : Laptop
            const on = TOPICS.filter((t) => s.topics[t.key])
            return (
              <tr key={s.id}>
                <td>
                  <span className="wb-row">
                    <Icon size={14} />
                    <span className="wb-ellipsis" title={d?.userAgent}>
                      {name}
                    </span>
                    {s.current && <Badge tone="accent">this device</Badge>}
                  </span>
                </td>
                <td className="wb-muted">{s.service}</td>
                <td className="wb-muted" title={on.map((t) => t.label).join(', ')}>
                  {on.length === TOPICS.length ? 'Everything' : on.length ? `${on.length} of ${TOPICS.length} kinds` : 'Nothing'}
                </td>
                <td>
                  <LastDelivery sub={s} />
                </td>
                <td className="actions">
                  <TestButton sub={s} label="Test" size="small" variant="ghost" />
                  <Button size="small" variant="ghost" icon={Trash2} onClick={() => void remove(s, name)}>
                    Remove
                  </Button>
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

/** The phone's More tab: push for this phone in a few rows. */
export function PushMobile() {
  const push = usePushInfo()
  const hostDesktop = useHostHasDesktop()
  if (push.error) return <ErrorBox error={push.error} onRetry={() => void push.refetch()} />
  if (!push.data) return <Loading />
  const info = push.data
  const mine = info.subscriptions.find((s) => s.current)
  const support = currentSupport()
  const denied = support.ok && notificationPermission() === 'denied' && !mine
  return (
    <div className="wb-set-box">
      <div className="wb-more-row">
        <span className="wb-grow">
          <div>Push notifications</div>
          <div className="wb-xs wb-subtle">
            {mine ? `On · via ${mine.service}` : support.ok ? (denied ? 'Blocked in the browser’s site settings' : 'Off') : 'Not available here'}
          </div>
        </span>
        {support.ok && !denied && <OnOffButton info={info} mine={mine} anyway={hostDesktop} />}
      </div>
      {(!support.ok || (hostDesktop && !denied)) && (
        <div className="wb-more-row">{support.ok ? <HostDesktopNote on={!!mine} /> : <Unsupported />}</div>
      )}
      {mine && (
        <>
          <div className="wb-more-row">
            <TopicChecks sub={mine} />
          </div>
          <div className="wb-more-row">
            <span className="wb-grow">
              <LastDelivery sub={mine} />
            </span>
            <TestButton sub={mine} label="Test" />
          </div>
        </>
      )}
    </div>
  )
}
