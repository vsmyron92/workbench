import { useState } from 'react'
import { Pencil, Plus, Trash2 } from 'lucide-react'
import { confirmDialog, toastError } from '@/shell/actions'
import { Badge, Button, Checkbox, EmptyState, IconButton, Input, Modal, Select } from '@/ui'
import { patchSettings, reportApply, useSettings } from '../api'
import { Group, Note } from '../common'
import { ACCOUNT_KINDS, accountHomeError, accountIdError, accountKind, suggestAccountId, type AccountKind } from '../lib'
import type { ProviderSettings } from '../types'

type Providers = Record<string, ProviderSettings>

/** The accounts of `providers`: entries of a kind that keeps its login in a folder, minus the built-in presets. */
function accountRows(providers: Providers) {
  return Object.entries(providers)
    .filter(([id, c]) => accountKind(c.kind) && !ACCOUNT_KINDS.some((k) => k.kind === id))
    .map(([id, c]) => {
      const k = accountKind(c.kind)!
      return { id, config: c, kind: k, home: c.env?.[k.homeVar] ?? '' }
    })
}

function AccountEditor({
  id: editId,
  initial,
  rows,
  onClose,
  onSave,
}: {
  id: string | null
  initial: ProviderSettings | null
  rows: ReturnType<typeof accountRows>
  onClose: () => void
  onSave: (id: string, config: ProviderSettings) => Promise<void>
}) {
  const editing = editId !== null
  const [kind, setKind] = useState<AccountKind>((accountKind(initial?.kind)?.kind ?? 'claude') as AccountKind)
  const k = accountKind(kind)!
  const [label, setLabel] = useState(initial?.label ?? '')
  const [id, setId] = useState(editId ?? '')
  const [idTouched, setIdTouched] = useState(editing)
  const [home, setHome] = useState(initial?.env?.[k.homeVar] ?? '')
  const [homeTouched, setHomeTouched] = useState(editing)
  const [enabled, setEnabled] = useState(initial?.enabled !== false)
  const [busy, setBusy] = useState(false)

  const effectiveId = idTouched ? id : suggestAccountId(kind, label)
  const effectiveHome = homeTouched ? home : effectiveId ? `${k.defaultHome}-${effectiveId.replace(`${kind}-`, '')}` : ''
  const others = rows.filter((r) => r.id !== editId && r.kind.kind === kind).map((r) => ({ id: r.id, home: r.home }))
  const idError = editing ? null : accountIdError(effectiveId, rows.map((r) => r.id))
  const homeError = accountHomeError(kind, effectiveHome, others)
  const error = idError ?? homeError

  const submit = async () => {
    if (error) return
    setBusy(true)
    try {
      // Everything else the section holds (model, args, command…) is kept as it is.
      const config: ProviderSettings = {
        ...initial,
        kind,
        label: label.trim() || null,
        enabled: enabled ? null : false,
        env: { ...initial?.env, [k.homeVar]: effectiveHome.trim() },
      }
      await onSave(effectiveId, config)
      onClose()
    } catch {
      // reported by the caller
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      title={editing ? `Account “${editId}”` : 'Add an account'}
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
      <div className="wb-small wb-muted">
        Another login of the same CLI, for a second subscription. Its sessions, history and settings live in their own folder, next to the default one.
      </div>
      <label className="wb-small wb-muted">CLI</label>
      <Select value={kind} disabled={editing} onChange={(e) => setKind(e.target.value as AccountKind)}>
        {ACCOUNT_KINDS.map((x) => (
          <option key={x.kind} value={x.kind}>
            {x.label}
          </option>
        ))}
      </Select>
      <label className="wb-small wb-muted">Label</label>
      <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Work" autoFocus={!editing} />
      <div className="wb-small wb-muted">Shown in the new session picker and on sessions. Empty: the name below.</div>
      <label className="wb-small wb-muted">Name</label>
      <Input
        className="mono"
        value={effectiveId}
        disabled={editing}
        onChange={(e) => {
          setId(e.target.value)
          setIdTouched(true)
        }}
        placeholder={`${kind}-work`}
      />
      <label className="wb-small wb-muted">Folder</label>
      <Input
        className="mono"
        value={effectiveHome}
        onChange={(e) => {
          setHome(e.target.value)
          setHomeTouched(true)
        }}
        placeholder={`${k.defaultHome}-work`}
      />
      <div className="wb-small wb-muted">
        Becomes <code>{k.homeVar}</code> for this account’s sessions. The CLI creates it and asks you to sign in the first time it runs there.
      </div>
      {editing && (
        <Checkbox checked={enabled} onChange={setEnabled}>
          Offer this account when starting a session
        </Checkbox>
      )}
      {error && (effectiveId || effectiveHome) && <div className="wb-field-error">{error}</div>}
    </Modal>
  )
}

/** Extra logins of Claude Code, Codex and Kimi Code: `[agents.providers.<name>]` with the CLI's own config folder. */
export function AccountsGroup() {
  const settings = useSettings()
  const [editor, setEditor] = useState<{ id: string | null; config: ProviderSettings | null } | null>(null)
  const providers: Providers = settings.data?.config.agents.providers ?? {}
  const rows = accountRows(providers)

  const save = async (next: Providers, what: string) => {
    try {
      reportApply(await patchSettings({ agents: { providers: next } }, settings.data?.hash), what)
    } catch (e) {
      toastError(e, 'Could not save')
      throw e
    }
  }

  const remove = async (id: string, label: string) => {
    const ok = await confirmDialog({
      title: `Remove account “${label}”?`,
      message:
        'Its sessions that are still open keep running, but cannot be restarted or resumed from Workbench. Its folder and login are left alone: add the account again to get them back.',
      confirmLabel: 'Remove',
      danger: true,
    })
    if (!ok) return
    const next = { ...providers }
    delete next[id]
    await save(next, `Account ${label} removed`).catch(() => {})
  }

  if (!settings.data) return null
  return (
    <>
      <Group
        title="Accounts"
        description="Run Claude Code, Codex or Kimi Code under more than one login, such as a work and a personal subscription. Each account keeps its login, history and settings in its own folder, and appears next to the CLI when you start a session."
        actions={
          <Button icon={Plus} onClick={() => setEditor({ id: null, config: null })}>
            Add account
          </Button>
        }
        flush
      >
        {rows.length === 0 ? (
          <div className="wb-set-box">
            <EmptyState title="Only the default accounts">
              Sessions use whichever login each CLI has outside Workbench (<code>~/.claude</code>, <code>~/.codex</code>…).
            </EmptyState>
          </div>
        ) : (
          <div className="wb-set-box" style={{ overflowX: 'auto' }}>
            <table className="wb-pf-table">
              <thead>
                <tr>
                  <th>Account</th>
                  <th>CLI</th>
                  <th>Folder</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => {
                  const label = r.config.label?.trim() || r.id
                  return (
                    <tr key={r.id}>
                      <td>
                        <div>{label}</div>
                        <div className="mono wb-subtle">{r.id}</div>
                      </td>
                      <td>
                        {r.kind.label}
                        {r.config.enabled === false && (
                          <>
                            {' '}
                            <Badge>hidden</Badge>
                          </>
                        )}
                      </td>
                      <td className="mono">{r.home || <span className="wb-subtle">not set</span>}</td>
                      <td>
                        <div className="wb-row">
                          <IconButton icon={Pencil} label="Edit" onClick={() => setEditor({ id: r.id, config: r.config })} />
                          <IconButton icon={Trash2} label="Remove" onClick={() => void remove(r.id, label)} />
                        </div>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}
      </Group>
      <div style={{ marginTop: 12 }}>
        <Note>
          To sign an account in, start a session with it: its CLI shows its own login the first time it runs in the new folder. Accounts are
          saved as <code>[agents.providers.&lt;name&gt;]</code> in <code>config.toml</code>, where model, effort and extra arguments can be set per account.
        </Note>
      </div>
      {editor && (
        <AccountEditor
          id={editor.id}
          initial={editor.config}
          rows={rows}
          onClose={() => setEditor(null)}
          onSave={(id, config) => save({ ...providers, [id]: config }, editor.id ? `Account ${config.label?.trim() || id} saved` : `Account ${config.label?.trim() || id} added`)}
        />
      )}
    </>
  )
}
