// Shown when there is no valid session. Normally you arrive signed in through
// `workbench open` (or the URL printed at startup); on another device, pair it
// from Settings → Remote access, or paste the token.

import { useState } from 'react'
import { KeyRound } from 'lucide-react'
import { api, setDeviceKey } from '@/api/client'
import { Button, Input } from '@/ui'

export function Login({ onDone }: { onDone: () => void }) {
  const [token, setToken] = useState('')
  const [error, setError] = useState<string | null>(() => {
    const p = new URLSearchParams(location.search).get('login')
    return p === 'failed' ? 'That login link is no longer valid.' : p === 'pair-failed' ? 'That pairing code is invalid or expired.' : null
  })
  const [busy, setBusy] = useState(false)
  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const r = await api.post<{ key?: string }>('/api/auth/login', { token: token.trim() })
      setDeviceKey(r.key ?? null)
      history.replaceState(null, '', '/')
      onDone()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="wb-login">
      <div className="wb-login-card">
        <img src="/favicon.svg" width={40} height={40} alt="" />
        <h1>Workbench</h1>
        <p className="wb-muted">
          On this computer, run <code>workbench open</code>. On another device, pair it from <b>Settings → Remote access</b> on a
          signed-in device, or paste the access token.
        </p>
        <form
          className="wb-row"
          onSubmit={(e) => {
            e.preventDefault()
            void submit()
          }}
        >
          <Input
            type="password"
            placeholder="Access token"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            autoFocus
            style={{ flex: 1 }}
          />
          <Button variant="primary" icon={KeyRound} loading={busy} disabled={!token.trim()} type="submit">
            Sign in
          </Button>
        </form>
        {error && <div className="wb-danger wb-small">{error}</div>}
      </div>
    </div>
  )
}
