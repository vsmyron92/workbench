// "Pair a device": mint a one-time code, show it as a QR code with a countdown,
// and watch the device list until the new device signs in.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { create } from 'zustand'
import { CheckCircle2, Copy, QrCode, RefreshCw, Smartphone } from 'lucide-react'
import { api } from '@/api/client'
import { toastError } from '@/shell/actions'
import { Button, Input, Modal, Select } from '@/ui'
import { pk, useRemote } from './api'
import { copyText, Note } from './common'
import { formatCountdown, hostNameOf, hostOf } from './lib'
import type { DeviceInfo, PairResult } from './types'

export const usePairDialog = create<{ open: boolean; show: () => void; hide: () => void }>()((set) => ({
  open: false,
  show: () => set({ open: true }),
  hide: () => set({ open: false }),
}))

export function openPairDialog() {
  usePairDialog.getState().show()
}

/** Mounted once by the platform provider. */
export function PairDialogHost() {
  const open = usePairDialog((s) => s.open)
  return open ? <PairDialog onClose={() => usePairDialog.getState().hide()} /> : null
}

function PairDialog({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [pair, setPair] = useState<PairResult | null>(null)
  const [deadline, setDeadline] = useState(0)
  const [now, setNow] = useState(Date.now())
  const [paired, setPaired] = useState<DeviceInfo | null>(null)
  const known = useRef<Set<string> | null>(null)
  const waiting = !!pair && !paired && deadline > now
  const remote = useRemote({ refetchInterval: waiting ? 2500 : undefined })

  useEffect(() => {
    if (!pair || paired) return
    const t = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(t)
  }, [pair, paired])

  // A device that was not there when the code was minted has redeemed it.
  useEffect(() => {
    const devices = remote.data?.devices
    if (!pair || !devices || !known.current) return
    const fresh = devices.find((d) => !known.current!.has(d.id))
    if (fresh) setPaired(fresh)
  }, [remote.data, pair])

  const generate = async (baseUrl?: string) => {
    setBusy(true)
    try {
      const current = await qc.fetchQuery({ queryKey: pk.remote, queryFn: () => api.get<{ devices: DeviceInfo[] }>('/api/platform/remote'), staleTime: 0 })
      known.current = new Set(current.devices.map((d) => d.id))
      const r = await api.post<PairResult>('/api/platform/pair', { name: name.trim() || undefined, baseUrl })
      setPair(r)
      setPaired(null)
      setNow(Date.now())
      // Count down from the server's TTL, not its clock: the two machines may disagree.
      setDeadline(Date.now() + r.ttlMs)
    } catch (e) {
      toastError(e, 'Could not create a pairing code')
    } finally {
      setBusy(false)
    }
  }

  const expired = !!pair && !paired && deadline <= now
  const chosen = pair?.candidates.find((c) => c.url === pair.baseUrl)
  const svg = useMemo(() => (pair ? { __html: pair.qrSvg } : undefined), [pair])

  const allowHost = async () => {
    const host = hostNameOf(pair?.baseUrl ?? '')
    if (!host) return
    try {
      await api.put('/api/platform/remote', { addAllowedHost: host })
      await qc.invalidateQueries({ queryKey: pk.all })
      await generate(pair?.baseUrl)
    } catch (e) {
      toastError(e, 'Could not update allowed hosts')
    }
  }

  return (
    <Modal
      title={
        <span className="wb-row">
          <Smartphone size={16} /> Pair a device
        </span>
      }
      wide
      onClose={onClose}
      footer={
        paired ? (
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        ) : (
          <>
            <Button onClick={onClose}>Close</Button>
            {pair && (
              <Button icon={RefreshCw} loading={busy} onClick={() => void generate(pair.baseUrl)}>
                New code
              </Button>
            )}
            {!pair && (
              <Button variant="primary" icon={QrCode} loading={busy} onClick={() => void generate()}>
                Create pairing code
              </Button>
            )}
          </>
        )
      }
    >
      {paired ? (
        <div className="wb-pair-done">
          <CheckCircle2 size={36} />
          <div style={{ fontWeight: 600, fontSize: 'var(--fs-lg)' }}>{paired.name} is paired</div>
          <div className="wb-muted wb-small">It stays signed in for 30 days of inactivity. Revoke it any time under Remote access.</div>
        </div>
      ) : !pair ? (
        <>
          <div className="wb-muted" style={{ lineHeight: 1.5 }}>
            A pairing code signs in one more device (a phone, a laptop) with its own session that you can revoke. The code works
            once and expires after 10 minutes.
          </div>
          <label className="wb-small wb-muted">Device name (optional)</label>
          <Input
            placeholder="e.g. Pixel phone"
            value={name}
            maxLength={60}
            autoFocus
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && void generate()}
          />
        </>
      ) : (
        <div className="wb-pair">
          <div className={expired ? 'wb-qr expired' : 'wb-qr'} dangerouslySetInnerHTML={svg} aria-label="Pairing QR code" role="img" />
          <div className="wb-pair-side">
            <div className="wb-muted wb-small">Scan with the phone's camera, or open the link on the other device:</div>
            <div className="wb-row">
              <a href={pair.url} target="_blank" rel="noreferrer" className="mono wb-small wb-ellipsis" title={pair.url}>
                {pair.url}
              </a>
              <Button size="small" icon={Copy} onClick={() => void copyText(pair.url, 'Link copied')}>
                Copy
              </Button>
            </div>
            <div className="wb-row" style={{ gap: 12 }}>
              <span className="wb-pair-code">{pair.code}</span>
              <span className={expired ? 'wb-danger' : 'wb-muted'}>{expired ? 'Expired' : `expires in ${formatCountdown(deadline - now)}`}</span>
            </div>
            {pair.candidates.length > 1 && (
              <div className="wb-row">
                <span className="wb-small wb-muted">Address</span>
                <Select value={pair.baseUrl} onChange={(e) => void generate(e.target.value)} style={{ flex: 1 }}>
                  {pair.candidates.map((c) => (
                    <option key={c.url} value={c.url}>
                      {c.label} — {c.url}
                      {c.reachable ? '' : ' (not reachable from other devices)'}
                    </option>
                  ))}
                </Select>
              </div>
            )}
            {pair.warning && (
              <Note tone="warning">
                {pair.warning}
                {chosen && !chosen.hostAllowed && chosen.reachable && (
                  <div style={{ marginTop: 6 }}>
                    <Button size="small" onClick={() => void allowHost()}>
                      Allow {hostOf(pair.baseUrl)}
                    </Button>
                  </div>
                )}
              </Note>
            )}
            {!expired && (
              <div className="wb-small wb-muted">Waiting for the device to sign in…</div>
            )}
          </div>
        </div>
      )}
    </Modal>
  )
}
