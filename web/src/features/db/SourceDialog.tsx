// Add / edit a data source. Credentials are secret *names* (Settings › Secrets
// defines where each value lives: a file, an env var, a .env key, the keyring…);
// Workbench never takes a password typed into the browser.

import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { create } from 'zustand'
import { openSettings, toast, toastError } from '@/shell/actions'
import { Button, Checkbox, Field, Input, Modal, Select } from '@/ui'
import { dbApi, dbKeys, useDbSources, type DbSource } from './api'

type Draft = Omit<DbSource, 'origin'>

const EMPTY: Draft = { name: '', kind: 'postgres', host: 'localhost', port: 5432, database: '', user: '', password: '', url: '', sslmode: '', readOnly: false }

export const useSourceDialog = create<{ open: { pid: string; source: DbSource | null } | null; show: (pid: string, source: DbSource | null) => void; hide: () => void }>()((set) => ({
  open: null,
  show: (pid, source) => set({ open: { pid, source } }),
  hide: () => set({ open: null }),
}))

export function SourceDialogHost() {
  const open = useSourceDialog((s) => s.open)
  if (!open) return null
  return <SourceDialog key={`${open.pid}:${open.source?.name ?? ''}`} pid={open.pid} source={open.source} />
}

function SourceDialog({ pid, source }: { pid: string; source: DbSource | null }) {
  const hide = useSourceDialog((s) => s.hide)
  const qc = useQueryClient()
  const list = useDbSources(pid)
  const [d, setD] = useState<Draft>(source ? { ...source } : EMPTY)
  const [mode, setMode] = useState<'fields' | 'url'>(source?.url ? 'url' : 'fields')
  const [busy, setBusy] = useState(false)
  const names = list.data?.secretNames ?? []
  const fromRepo = source?.origin === 'repository'
  useEffect(() => {
    if (!source && !d.name && d.database) setD((x) => ({ ...x, name: x.database }))
  }, [d.database, d.name, source])
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => setD((x) => ({ ...x, [k]: v }))

  const save = async () => {
    const draft: Draft = mode === 'url' ? { ...d, password: '' } : { ...d, url: '' }
    if (!draft.name.trim()) return toast('warning', 'Give the data source a name')
    if (mode === 'url' && !draft.url) return toast('warning', 'Pick the secret that holds the connection URL')
    setBusy(true)
    try {
      await dbApi.putSource(pid, { ...draft, name: draft.name.trim() }, source && source.name !== draft.name.trim() ? source.name : undefined)
      await qc.invalidateQueries({ queryKey: dbKeys.sources(pid) })
      void qc.invalidateQueries({ queryKey: ['db', pid, 'catalog'] })
      hide()
      try {
        const t = await dbApi.test(pid, draft.name.trim())
        toast('success', `Connected to ${draft.name.trim()} in ${t.ms} ms`, { detail: `${t.version.split(',')[0]} · as ${t.user}${t.ssl ? ' · TLS' : ''}` })
      } catch (e) {
        toastError(e, `Saved, but ${draft.name.trim()} did not connect`)
      }
    } catch (e) {
      toastError(e, 'Could not save the data source')
    } finally {
      setBusy(false)
    }
  }

  const secretSelect = (value: string, onChange: (v: string) => void, none: string) => (
    <Select value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">{none}</option>
      {names.map((n) => (
        <option key={n} value={n}>
          {n}
        </option>
      ))}
      {value && !names.includes(value) && <option value={value}>{value} (not defined)</option>}
    </Select>
  )

  return (
    <Modal
      title={source ? `Edit ${source.name}` : 'Add Data Source'}
      onClose={hide}
      footer={
        <>
          <span className="wb-grow wb-small wb-subtle">Saved to {list.data?.overlayPath ?? 'the machine overlay'}</span>
          <Button onClick={hide}>Cancel</Button>
          <Button variant="primary" loading={busy} onClick={() => void save()}>
            Save and Test
          </Button>
        </>
      }
    >
      <div className="wb-db-form">
        {fromRepo && <div className="wb-small wb-subtle">This source comes from the repository's .workbench.toml; saving writes your own copy to the machine overlay, which replaces it.</div>}
        <Field label="Name">
          <Input value={d.name} onChange={(e) => set('name', e.target.value)} placeholder="dev" autoFocus />
        </Field>
        <Field label="Connect with">
          <Select value={mode} onChange={(e) => setMode(e.target.value as 'fields' | 'url')}>
            <option value="fields">Host, database and user (password from a secret)</option>
            <option value="url">A secret holding the whole connection URL</option>
          </Select>
        </Field>
        {mode === 'url' && (
          <Field label="URL secret" hint={<>A secret whose value is postgres://user:password@host:port/db — e.g. {'{ dotenv = ".env", key = "DATABASE_URL" }'}. Host, port, database and user below override its parts when set.</>}>
            {secretSelect(d.url, (v) => set('url', v), '— pick a secret —')}
          </Field>
        )}
        <div className="wb-db-form-row">
          <Field label="Host">
            <Input value={d.host} onChange={(e) => set('host', e.target.value)} placeholder={mode === 'url' ? 'from the URL' : 'localhost'} />
          </Field>
          <Field label="Port">
            <Input value={d.port ?? ''} onChange={(e) => set('port', e.target.value ? Number(e.target.value.replace(/\D/g, '')) || null : null)} placeholder="5432" inputMode="numeric" />
          </Field>
        </div>
        <div className="wb-db-form-row">
          <Field label="Database">
            <Input value={d.database} onChange={(e) => set('database', e.target.value)} placeholder={mode === 'url' ? 'from the URL' : 'the user name'} />
          </Field>
          <Field label="User">
            <Input value={d.user} onChange={(e) => set('user', e.target.value)} placeholder={mode === 'url' ? 'from the URL' : 'your login name'} />
          </Field>
        </div>
        {mode === 'fields' && (
          <Field
            label="Password secret"
            hint={
              <>
                The name of a [secrets] entry.{' '}
                <button type="button" className="wb-db-link" onClick={() => (hide(), openSettings('secrets'))}>
                  Settings › Secrets
                </button>{' '}
                shows them. Without one, ~/.pgpass is used (or the server's trust / peer authentication).
              </>
            }
          >
            {secretSelect(d.password, (v) => set('password', v), '— none —')}
          </Field>
        )}
        <div className="wb-db-form-row">
          <Field label="SSL" hint="prefer and require encrypt without checking the certificate (as libpq); verify-full checks it against the system's roots.">
            <Select value={d.sslmode || 'prefer'} onChange={(e) => set('sslmode', e.target.value === 'prefer' ? '' : e.target.value)}>
              <option value="disable">disable</option>
              <option value="prefer">prefer</option>
              <option value="require">require</option>
              <option value="verify-full">verify-full</option>
            </Select>
          </Field>
          <Field label="Safety">
            <Checkbox checked={d.readOnly} onChange={(v) => set('readOnly', v)}>
              Read-only sessions
            </Checkbox>
          </Field>
        </div>
      </div>
    </Modal>
  )
}
