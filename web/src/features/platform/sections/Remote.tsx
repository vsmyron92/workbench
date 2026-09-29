import { useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Laptop, LogOut, QrCode, RotateCcw, Save, ShieldAlert, ShieldCheck, Smartphone } from 'lucide-react'
import { api } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Badge, Button, Checkbox, EmptyState, ErrorBox, Input, Loading, StatusDot, TimeAgo } from '@/ui'
import { pk, reportApply, useRemote, useSettings } from '../api'
import { CodeLine, Group, Note, Page, Row, StringList, useDraft } from '../common'
import { hostEntryError, hostNameOf, inConfigDir, publicUrlError, restartText } from '../lib'
import { openPairDialog } from '../PairDialog'
import type { ApplyResult, DeviceInfo, RemoteInfo } from '../types'

interface Form {
  bind: string
  allowedHosts: string[]
  publicUrl: string
  tlsOn: boolean
  cert: string
  key: string
}

function fromRemote(r: RemoteInfo): Form {
  return {
    bind: r.configuredBind,
    allowedHosts: r.allowedHosts,
    publicUrl: r.publicUrl ?? '',
    tlsOn: r.tls.configured,
    cert: r.tls.cert ?? '',
    key: r.tls.key ?? '',
  }
}

const normalize = (f: Form) => ({
  bind: f.bind.trim(),
  allowedHosts: f.allowedHosts,
  publicUrl: f.publicUrl.trim().replace(/\/+$/, ''),
  tls: f.tlsOn ? { cert: f.cert.trim(), key: f.key.trim() } : null,
})

function deviceKind(d: DeviceInfo) {
  return /Android|iOS/.test(d.name) || /Mobile|Android|iPhone/.test(d.userAgent) ? Smartphone : Laptop
}

/** Paired devices with revoke; the current one is marked. Shared with the phone "More" tab. */
export function DevicesList({ devices, compact }: { devices: DeviceInfo[]; compact?: boolean }) {
  const qc = useQueryClient()
  const revoke = async (d: DeviceInfo) => {
    const ok = await confirmDialog({
      title: d.current ? 'Sign out this device?' : `Revoke “${d.name}”?`,
      message: d.current
        ? 'This browser will need the token or a new pairing code to sign in again.'
        : 'That device is signed out immediately and needs a new pairing code to come back.',
      confirmLabel: d.current ? 'Sign out' : 'Revoke',
      danger: true,
    })
    if (!ok) return
    try {
      await api.del(`/api/auth/devices/${encodeURIComponent(d.id)}`)
      if (d.current) {
        location.reload()
        return
      }
      toast('success', `${d.name} revoked`)
      await qc.invalidateQueries({ queryKey: pk.remote })
    } catch (e) {
      toastError(e, 'Could not revoke')
    }
  }
  const sorted = [...devices].sort((a, b) => Number(b.current) - Number(a.current) || b.lastSeenAt - a.lastSeenAt)
  if (!sorted.length) return <EmptyState title="No device sessions" />
  return (
    <table className="wb-pf-table">
      {!compact && (
        <thead>
          <tr>
            <th>Device</th>
            <th>Connection</th>
            <th>Last seen</th>
            <th>Signed in</th>
            <th />
          </tr>
        </thead>
      )}
      <tbody>
        {sorted.map((d) => {
          const I = deviceKind(d)
          return (
            <tr key={d.id}>
              <td>
                <span className="wb-row">
                  <I size={14} />
                  <span className="wb-ellipsis" title={d.userAgent}>
                    {d.name}
                  </span>
                  {d.current && <Badge tone="accent">this device</Badge>}
                </span>
                {compact && (
                  <div className="wb-xs wb-subtle">
                    {d.remote ? 'remote' : 'local'} · seen <TimeAgo time={d.lastSeenAt} />
                  </div>
                )}
              </td>
              {!compact && (
                <>
                  <td>{d.remote ? <Badge tone="warning">remote</Badge> : <Badge>this computer</Badge>}</td>
                  <td className="wb-muted">
                    <TimeAgo time={d.lastSeenAt} />
                  </td>
                  <td className="wb-muted">{new Date(d.createdAt).toLocaleDateString()}</td>
                </>
              )}
              <td className="actions">
                <Button size="small" variant={compact ? 'default' : 'ghost'} icon={d.current ? LogOut : undefined} onClick={() => void revoke(d)}>
                  {d.current ? 'Sign out' : 'Revoke'}
                </Button>
              </td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}

export function RemoteSection() {
  const qc = useQueryClient()
  const remote = useRemote()
  const data = remote.data
  const saved = useMemo(() => (data ? fromRemote(data) : undefined), [data])
  const { draft: form, setDraft: setForm, dirty, reset } = useDraft(saved, normalize)
  const [busy, setBusy] = useState(false)
  // The certificate's suggested place: the config dir this server reads.
  const configDir = useSettings().data?.paths.configDir

  if (remote.error) return <ErrorBox error={remote.error} onRetry={() => void remote.refetch()} />
  if (!data || !form) return <Loading />

  const set = (p: Partial<Form>) => setForm({ ...form, ...p })
  const urlError = publicUrlError(form.publicUrl)
  const put = async (body: Record<string, unknown>, what: string) => {
    const r = await api.put<{ applied: ApplyResult; remote: RemoteInfo }>('/api/platform/remote', body)
    qc.setQueryData(pk.remote, r.remote)
    void qc.invalidateQueries({ queryKey: pk.settings })
    reportApply(r.applied, what)
  }
  const save = async () => {
    setBusy(true)
    try {
      const n = normalize(form)
      await put({ bind: n.bind, allowedHosts: n.allowedHosts, publicUrl: n.publicUrl || null, tls: n.tls }, 'Remote access saved')
    } catch (e) {
      toastError(e, 'Could not save')
    } finally {
      setBusy(false)
    }
  }
  const allow = async (host: string) => {
    try {
      await put({ addAllowedHost: host }, `${host} allowed`)
    } catch (e) {
      toastError(e, 'Could not update allowed hosts')
    }
  }

  const listening = data.addresses.filter((a) => a.listening)
  const bindText = data.loopbackOnly ? 'this computer only' : data.bind.startsWith('0.0.0.0') || data.bind.startsWith('[::]') ? 'all network interfaces' : 'one network address'

  return (
    <Page
      title="Remote access"
      wide
      description="Use Workbench from a phone or another computer. Every device signs in with a pairing code and can be revoked; anyone signed in can run commands on this machine."
      actions={
        <Button variant="primary" icon={QrCode} onClick={openPairDialog}>
          Pair a device
        </Button>
      }
    >
      {data.restartRequired.length > 0 && (
        <div style={{ marginTop: 12 }}>
          <Note tone="warning">Restart Workbench to apply the new {restartText(data.restartRequired)}.</Note>
        </div>
      )}

      <Group title="Status">
        <Row label="Listening on">
          <span className="mono">{data.bind}</span>
          <Badge tone={data.loopbackOnly ? undefined : 'warning'}>{bindText}</Badge>
        </Row>
        <Row label="Encryption" hint="Remote devices should use HTTPS: through a proxy (recommended) or Workbench's own TLS.">
          {data.tls.active ? (
            <span className="wb-row wb-success">
              <ShieldCheck size={14} /> HTTPS served by Workbench
            </span>
          ) : data.publicUrl?.startsWith('https://') ? (
            <span className="wb-row wb-success">
              <ShieldCheck size={14} /> HTTPS through {hostNameOf(data.publicUrl)}
            </span>
          ) : data.loopbackOnly ? (
            <span className="wb-muted">Not needed while Workbench stays on this computer</span>
          ) : (
            <span className="wb-row wb-warning">
              <ShieldAlert size={14} /> Plain HTTP on the network — prefer a proxy with HTTPS
            </span>
          )}
        </Row>
        <Row label="This browser" hint="How the page you are looking at reached Workbench.">
          <span className="mono">{data.requestHost}</span>
          {data.secure ? <Badge tone="success">https</Badge> : <Badge>http</Badge>}
        </Row>
      </Group>

      <Group
        title="Network addresses"
        flush
        description={
          listening.length
            ? 'Other devices on the same network (or tailnet) can open these.'
            : 'Workbench does not listen on these yet. Set the bind address below (for example 0.0.0.0:7777, or just the Tailscale address) and restart.'
        }
      >
        <div className="wb-set-box">
          {data.addresses.length === 0 ? (
            <EmptyState title="No network interfaces found" />
          ) : (
            <table className="wb-pf-table">
              <thead>
                <tr>
                  <th>Interface</th>
                  <th>Address</th>
                  <th>Type</th>
                  <th>Status</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {data.addresses.map((a) => (
                  <tr key={a.url}>
                    <td className="mono">{a.interface}</td>
                    <td className="mono">{a.url}</td>
                    <td>
                      <Badge tone={a.kind === 'tailscale' ? 'accent' : undefined}>{a.kind === 'lan' ? 'LAN' : a.kind === 'tailscale' ? 'Tailscale' : 'virtual'}</Badge>
                    </td>
                    <td>
                      {!a.listening ? (
                        <span className="wb-row wb-muted">
                          <StatusDot /> not listening
                        </span>
                      ) : !a.hostAllowed ? (
                        <span className="wb-row wb-warning">
                          <StatusDot tone="warning" /> host not allowed
                        </span>
                      ) : (
                        <span className="wb-row wb-success">
                          <StatusDot tone="success" /> reachable
                        </span>
                      )}
                    </td>
                    <td className="actions">
                      {a.listening && !a.hostAllowed && (
                        <Button size="small" onClick={() => void allow(a.address)}>
                          Allow
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </Group>

      <Group
        title="Settings"
        actions={
          <>
            <Button size="small" icon={RotateCcw} disabled={!dirty || busy} onClick={reset}>
              Revert
            </Button>
            <Button size="small" variant="primary" icon={Save} disabled={!dirty || !!urlError} loading={busy} onClick={() => void save()}>
              Save
            </Button>
          </>
        }
      >
        <Row label="Bind address" hint="127.0.0.1:PORT keeps Workbench local. Changing it needs a restart.">
          <Input className="mono" value={form.bind} onChange={(e) => set({ bind: e.target.value })} placeholder="127.0.0.1:7777" />
        </Row>
        <Row
          top
          label="Allowed hosts"
          hint="Host names other devices use (besides loopback and the bind address). Requests for any other Host are refused, which blocks DNS-rebinding attacks."
        >
          <StringList
            value={form.allowedHosts}
            onChange={(allowedHosts) => set({ allowedHosts })}
            placeholder="box.tailnet.ts.net or 192.168.1.20"
            validate={hostEntryError}
          />
        </Row>
        <Row label="Public URL" hint="The address other devices use — put in pairing links and QR codes. Its host is allowed automatically.">
          <Input value={form.publicUrl} onChange={(e) => set({ publicUrl: e.target.value })} placeholder="https://box.tailnet.ts.net" />
          {urlError && <span className="wb-field-error">{urlError}</span>}
        </Row>
        <Row label="Serve HTTPS" hint="PEM certificate and key (e.g. from `tailscale cert`). Plain HTTP keeps working from this computer. Needs a restart.">
          <Checkbox checked={form.tlsOn} onChange={(tlsOn) => set({ tlsOn })}>
            Use my certificate
          </Checkbox>
        </Row>
        {form.tlsOn && (
          <>
            <Row label="Certificate" hint={data.tls.configured && data.tls.certExists === false ? 'File not found' : undefined}>
              <Input className="mono" value={form.cert} onChange={(e) => set({ cert: e.target.value })} placeholder={inConfigDir(configDir, 'tls', 'cert.pem')} />
            </Row>
            <Row label="Private key" hint={data.tls.configured && data.tls.keyExists === false ? 'File not found' : undefined}>
              <Input className="mono" value={form.key} onChange={(e) => set({ key: e.target.value })} placeholder={inConfigDir(configDir, 'tls', 'key.pem')} />
            </Row>
          </>
        )}
      </Group>

      <Group title="Recommended: keep Workbench local and publish it through a proxy" flush>
        <div className="wb-set-box" style={{ padding: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div className="wb-small wb-muted">
            <b>Tailscale</b> gives you HTTPS on your tailnet without opening any port. Run this once, then set the public URL to{' '}
            <code>https://&lt;machine&gt;.&lt;tailnet&gt;.ts.net</code>:
          </div>
          <CodeLine text={`tailscale serve --bg --https=443 http://127.0.0.1:${data.port}`} />
          <div className="wb-small wb-muted" style={{ marginTop: 4 }}>
            <b>Caddy</b> on your own domain (add the domain as the public URL):
          </div>
          <CodeLine text={`workbench.example.com {\n  reverse_proxy 127.0.0.1:${data.port}\n}`.replace(/\n\s*/g, ' ')} />
        </div>
      </Group>

      <Group
        title={`Devices (${data.devices.length})`}
        flush
        actions={
          <Button size="small" icon={QrCode} onClick={openPairDialog}>
            Pair a device
          </Button>
        }
      >
        <div className="wb-set-box">
          <DevicesList devices={data.devices} />
        </div>
      </Group>
    </Page>
  )
}
