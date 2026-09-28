import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { RefreshCw, RotateCcw, Save } from 'lucide-react'
import { ApiError, api } from '@/api/client'
import { toast, toastError } from '@/shell/actions'
import { Button, ErrorBox, Loading, Toolbar } from '@/ui'
import { pk, reportApply, useRawConfig } from '../api'
import { DiagnosticLine, TomlEditor, useDraft, useTomlDiagnostics } from '../common'
import type { ApplyResult } from '../types'

export function RawConfigSection() {
  const qc = useQueryClient()
  const raw = useRawConfig()
  const { draft, setDraft, dirty, reset } = useDraft<string>(raw.data?.text)
  const { diag, checking } = useTomlDiagnostics(draft, 'global')
  const [busy, setBusy] = useState(false)

  if (raw.error) return <ErrorBox error={raw.error} onRetry={() => void raw.refetch()} />
  if (!raw.data || draft === null) return <Loading />

  const save = async (force = false) => {
    if (!dirty || busy) return
    setBusy(true)
    try {
      const r = await api.put<ApplyResult>('/api/settings/raw', { text: draft, baseHash: force ? undefined : raw.data?.hash })
      // Adopt the saved text as the new baseline before the refetch lands.
      qc.setQueryData(pk.raw, { ...raw.data!, text: draft, hash: r.hash, exists: true })
      reportApply(r, 'config.toml saved and applied')
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toast('warning', 'config.toml changed on disk since you opened it.', {
          timeout: 0,
          action: { label: 'Overwrite it', run: () => void save(true) },
        })
      } else {
        toastError(e, 'Not saved')
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="wb-fill">
      <Toolbar title="config.toml">
        <span className="mono wb-small wb-muted wb-ellipsis" title={raw.data.path}>
          {raw.data.path}
        </span>
        {dirty && <span className="wb-small wb-warning">modified</span>}
        <span style={{ flex: 1 }} />
        <Button size="small" variant="ghost" icon={RefreshCw} disabled={busy} onClick={() => void raw.refetch()} title="Load the file from disk">
          Reload
        </Button>
        <Button size="small" icon={RotateCcw} disabled={!dirty || busy} onClick={reset}>
          Revert
        </Button>
        <Button
          size="small"
          variant="primary"
          icon={Save}
          disabled={!dirty || (diag !== null && !diag.ok)}
          loading={busy}
          onClick={() => void save()}
          title="Save and apply (Ctrl+S)"
        >
          Save and apply
        </Button>
      </Toolbar>
      <TomlEditor kind="global" fill value={draft} onChange={setDraft} diagnostic={diag} onSave={() => void save()} />
      <div style={{ padding: '2px 10px', borderTop: '1px solid var(--border)' }}>
        <DiagnosticLine diag={diag} checking={checking} />
      </div>
    </div>
  )
}
