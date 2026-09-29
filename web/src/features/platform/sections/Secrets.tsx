import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { KeyRound, Lock, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { api } from '@/api/client'
import { useHealth } from '@/api/health'
import { useProjects } from '@/api/queries'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Badge, Button, EmptyState, ErrorBox, IconButton, Input, Loading, Modal, Select, StatusDot } from '@/ui'
import { patchSettings, pk, reportApply, useSecrets, useSettings } from '../api'
import { Group, Note, Page } from '../common'
import { makeSecretRef, SECRET_NAME_RE, secretRefFields, type SecretSource } from '../lib'
import type { SecretRef, SecretRow } from '../types'

/** `windowsHint` replaces `hint` when the server runs on Windows. */
const SOURCES: { value: SecretSource; label: string; placeholder: string; hint: string; windowsHint?: string }[] = [
  {
    value: 'file',
    label: 'File',
    placeholder: '~/.gitlab_token',
    hint: 'A file containing only the value. Keep it private (chmod 600).',
    windowsHint: 'A file containing only the value. Keep it private: Make private lets only you (and SYSTEM) read it.',
  },
  { value: 'env', label: 'Environment variable', placeholder: 'GITLAB_TOKEN', hint: "Read from Workbench's own environment." },
  {
    value: 'keyring',
    label: 'Keyring',
    placeholder: 'workbench/gitlab',
    hint: 'service/account in the desktop keyring (Secret Service).',
    windowsHint: 'service/account in Windows Credential Manager (the generic credential account.service).',
  },
  { value: 'dotenv', label: '.env file', placeholder: '~/project/.env', hint: 'KEY=value file; enter the path and the key.' },
  { value: 'command', label: 'Command', placeholder: 'pass show gitlab', hint: 'Runs without a shell; its output is the value.' },
]

function SecretEditor({
  initialName,
  initial,
  existing,
  onClose,
  onSave,
}: {
  initialName: string
  initial: SecretRef | null
  existing: string[]
  onClose: () => void
  onSave: (name: string, ref: SecretRef) => Promise<void>
}) {
  const editing = !!initial
  const fields = initial ? secretRefFields(initial) : { source: 'file' as SecretSource, location: '', key: '' }
  const [name, setName] = useState(initialName)
  const [source, setSource] = useState<SecretSource>(fields.source)
  const [location, setLocation] = useState(fields.location)
  const [key, setKey] = useState(fields.key)
  const [busy, setBusy] = useState(false)
  const windows = useHealth()?.os === 'windows'
  const src = SOURCES.find((s) => s.value === source)!
  const ref = makeSecretRef(source, location, key)
  const nameError = !name.trim()
    ? 'Enter a name'
    : !SECRET_NAME_RE.test(name.trim())
      ? 'Letters, digits, - _ and . only'
      : !editing && existing.includes(name.trim())
        ? 'A secret with this name exists'
        : null
  const error = nameError ?? (typeof ref === 'string' ? ref : null)
  const submit = async () => {
    if (error || typeof ref === 'string') return
    setBusy(true)
    try {
      await onSave(name.trim(), ref)
      onClose()
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title={editing ? `Secret “${initialName}”` : 'Add a secret reference'}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!!error} loading={busy} onClick={() => void submit()}>
            Save
          </Button>
        </>
      }
    >
      <div className="wb-small wb-muted">Workbench stores where the value lives, never the value.</div>
      <label className="wb-small wb-muted">Name</label>
      <Input value={name} disabled={editing} onChange={(e) => setName(e.target.value)} placeholder="gitlab" autoFocus={!editing} />
      <label className="wb-small wb-muted">Source</label>
      <Select value={source} onChange={(e) => setSource(e.target.value as SecretSource)}>
        {SOURCES.map((s) => (
          <option key={s.value} value={s.value}>
            {s.label}
          </option>
        ))}
      </Select>
      <label className="wb-small wb-muted">{source === 'dotenv' ? 'Path' : source === 'command' ? 'Command' : 'Location'}</label>
      <Input className="mono" value={location} placeholder={src.placeholder} onChange={(e) => setLocation(e.target.value)} autoFocus={editing} />
      {source === 'dotenv' && (
        <>
          <label className="wb-small wb-muted">Key</label>
          <Input className="mono" value={key} placeholder="GITLAB_TOKEN" onChange={(e) => setKey(e.target.value)} />
        </>
      )}
      <div className="wb-small wb-muted">{(windows && src.windowsHint) || src.hint}</div>
      {error && (location || name) && <div className="wb-field-error">{error}</div>}
    </Modal>
  )
}

function StatusCell({ row }: { row: SecretRow }) {
  if (row.resolved) {
    return (
      <span className="wb-row">
        <StatusDot tone="success" /> Resolves
      </span>
    )
  }
  return (
    <span className="wb-row" title={row.error}>
      <StatusDot tone="danger" />
      <span className="wb-ellipsis wb-danger" style={{ maxWidth: 240 }}>
        {row.error ?? 'Not resolved'}
      </span>
    </span>
  )
}

export function SecretsSection() {
  const qc = useQueryClient()
  const secrets = useSecrets()
  const settings = useSettings()
  const { data: projects } = useProjects()
  const [editor, setEditor] = useState<{ name: string; ref: SecretRef | null } | null>(null)
  const [fixing, setFixing] = useState<string | null>(null)
  // The endpoint applies Windows' private access list there: no chmod to name.
  const makePrivate = useHealth()?.os === 'windows' ? 'Make private' : 'chmod 600'

  const globalRefs = settings.data?.config.secrets ?? {}
  const projectName = (id?: string) => projects?.find((p) => p.id === id)?.name ?? id ?? ''

  const saveRefs = async (next: Record<string, SecretRef>, what: string) => {
    try {
      reportApply(await patchSettings({ secrets: next }, settings.data?.hash), what)
      await qc.invalidateQueries({ queryKey: pk.secrets })
    } catch (e) {
      toastError(e, 'Could not save')
      throw e
    }
  }

  const chmod = async (row: SecretRow) => {
    const key = `${row.projectId ?? ''}/${row.name}`
    setFixing(key)
    try {
      await api.post(`/api/settings/secrets/${encodeURIComponent(row.name)}/chmod`, {}, { projectId: row.projectId })
      toast('success', `${row.location} is now private${makePrivate === 'chmod 600' ? ' (600)' : ''}`)
      await qc.invalidateQueries({ queryKey: pk.secrets })
    } catch (e) {
      toastError(e, 'Could not change permissions')
    } finally {
      setFixing(null)
    }
  }

  const remove = async (row: SecretRow) => {
    const ok = await confirmDialog({
      title: `Remove secret “${row.name}”?`,
      message: row.usedBy.length
        ? `It is still used by ${row.usedBy.join(', ')}. Those integrations stop working until you define it again.`
        : 'Only the reference is removed; the file, variable or keyring entry is left alone.',
      confirmLabel: 'Remove',
      danger: true,
    })
    if (!ok) return
    const next = { ...globalRefs }
    delete next[row.name]
    await saveRefs(next, `Secret ${row.name} removed`).catch(() => {})
  }

  const rows = secrets.data?.secrets ?? []
  const fixable = rows.filter((r) => r.fixable).length

  return (
    <Page
      title="Secrets"
      wide
      description="Tokens and passwords are referenced by name. The server reads the values when it needs them; they never reach the browser, command lines or logs."
      actions={
        <>
          <IconButton icon={RefreshCw} label="Re-check" onClick={() => void secrets.refetch()} />
          <Button icon={Plus} onClick={() => setEditor({ name: '', ref: null })}>
            Add reference
          </Button>
        </>
      }
    >
      {fixable > 0 && (
        <div style={{ marginTop: 12 }}>
          <Note tone="warning">
            {fixable === 1 ? 'One secret file is' : `${fixable} secret files are`} readable by other users on this machine. Use{' '}
            <b>{makePrivate}</b> to make {fixable === 1 ? 'it' : 'them'} private.
          </Note>
        </div>
      )}
      {(secrets.data?.missing.length ?? 0) > 0 && (
        <div style={{ marginTop: 12 }}>
          <Note tone="warning">
            <div>Referenced but not defined:</div>
            {secrets.data!.missing.map((m) => (
              <div key={`${m.projectId}/${m.name}`} className="wb-row" style={{ marginTop: 4 }}>
                <span className="mono">{m.name}</span>
                <span className="wb-muted wb-small">
                  {m.projectId ? `${projectName(m.projectId)}: ` : ''}
                  {m.usedBy.join(', ')}
                </span>
                <Button size="small" onClick={() => setEditor({ name: m.name, ref: null })}>
                  Define
                </Button>
              </div>
            ))}
          </Note>
        </div>
      )}
      <Group title="References" flush>
        {secrets.isLoading ? (
          <Loading label="Checking secrets…" />
        ) : secrets.error ? (
          <ErrorBox error={secrets.error} onRetry={() => void secrets.refetch()} />
        ) : rows.length === 0 ? (
          <div className="wb-set-box">
            <EmptyState icon={KeyRound} title="No secrets yet">
              Add a reference for your GitLab or Atlassian token, e.g. a file like <code>~/.gitlab_token</code>.
            </EmptyState>
          </div>
        ) : (
          <div className="wb-set-box" style={{ overflowX: 'auto' }}>
            <table className="wb-pf-table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Defined in</th>
                  <th>Source</th>
                  <th>Status</th>
                  <th>Permissions</th>
                  <th>Used by</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => (
                  <tr key={`${r.projectId ?? ''}/${r.name}`}>
                    <td className="mono">{r.name}</td>
                    <td>{r.scope === 'global' ? <Badge>config.toml</Badge> : <Badge tone="accent">{projectName(r.projectId)}</Badge>}</td>
                    <td>
                      <div>{SOURCES.find((s) => s.value === r.source)?.label ?? r.source}</div>
                      <div className="mono wb-subtle wb-ellipsis" style={{ maxWidth: 260 }} title={r.location}>
                        {r.location}
                      </div>
                    </td>
                    <td>
                      <StatusCell row={r} />
                    </td>
                    <td>
                      {r.mode ? (
                        <span className="wb-row">
                          <span className="mono">{r.mode}</span>
                          {r.fixable && (
                            <Badge tone="warning" title={r.warnings.join('\n')}>
                              readable by others
                            </Badge>
                          )}
                        </span>
                      ) : (
                        <span className="wb-subtle">—</span>
                      )}
                    </td>
                    <td className="wb-small wb-muted">{r.usedBy.length ? r.usedBy.join(', ') : <span className="wb-subtle">unused</span>}</td>
                    <td className="actions">
                      {r.fixable && (
                        <Button
                          size="small"
                          icon={Lock}
                          loading={fixing === `${r.projectId ?? ''}/${r.name}`}
                          onClick={() => void chmod(r)}
                        >
                          {makePrivate}
                        </Button>
                      )}
                      {r.scope === 'global' ? (
                        <>
                          <IconButton icon={Pencil} size="small" label="Edit" onClick={() => setEditor({ name: r.name, ref: globalRefs[r.name] ?? null })} />
                          <IconButton icon={Trash2} size="small" label="Remove" onClick={() => void remove(r)} />
                        </>
                      ) : (
                        <span className="wb-small wb-subtle" title="Edit it in the project's configuration">
                          project config
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Group>
      {editor && (
        <SecretEditor
          initialName={editor.name}
          initial={editor.ref}
          existing={Object.keys(globalRefs)}
          onClose={() => setEditor(null)}
          onSave={(name, ref) => saveRefs({ ...globalRefs, [name]: ref }, `Secret ${name} saved`)}
        />
      )}
    </Page>
  )
}
