import { useState } from 'react'
import { ArrowDown, ArrowUp, Pencil, Plus, RefreshCw, Trash2, X } from 'lucide-react'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Badge, Button, Checkbox, EmptyState, IconButton, Input, Modal, Select } from '@/ui'
import { patchSettings, probeLocalModels, reportApply, setAccountLimit, useAccountUsage, useSettings } from '../api'
import { Group, Note, Row } from '../common'
import {
  ACCOUNT_KINDS,
  accountHomeError,
  accountIdError,
  accountKind,
  accountName,
  fallbackCandidates,
  formatUntil,
  LOCAL_BY_KIND,
  LOCAL_SERVERS,
  localUrlError,
  moveItem,
  suggestAccountId,
  suggestLocalId,
  usageTone,
  type AccountKind,
} from '../lib'
import type { AccountUsage, FailoverMode, ProviderSettings } from '../types'

type Providers = Record<string, ProviderSettings>

/** The built-in CLIs: always there, configured only by what they fall back to. */
const BUILT_IN = ['claude', 'codex', 'kimi', 'gemini', 'aider']
const ALWAYS_LISTED = ['claude', 'codex']

interface AccountRow {
  id: string
  config: ProviderSettings
  builtIn: boolean
  kind: (typeof ACCOUNT_KINDS)[number] | undefined
  home: string
}

/** Every account to list: the built-in CLIs that are used, then the extra accounts and local models. */
function accountRows(providers: Providers): AccountRow[] {
  const extra: AccountRow[] = Object.entries(providers)
    .filter(([id, c]) => accountKind(c.kind) && !BUILT_IN.includes(id))
    .map(([id, c]) => ({ id, config: c, builtIn: false, kind: accountKind(c.kind), home: c.env?.[accountKind(c.kind)!.homeVar] ?? '' }))
  const used = new Set<string | undefined>(extra.map((r) => r.kind?.kind))
  const builtIn: AccountRow[] = BUILT_IN.filter((id) => ALWAYS_LISTED.includes(id) || providers[id] || used.has(id)).map((id) => ({
    id,
    config: providers[id] ?? {},
    builtIn: true,
    kind: accountKind(id),
    home: '',
  }))
  return [...builtIn, ...extra]
}

/** `config` without the keys that say nothing, so config.toml stays short. */
function tidy(c: ProviderSettings): ProviderSettings {
  const out: ProviderSettings = { ...c }
  for (const k of Object.keys(out) as (keyof ProviderSettings)[]) {
    const v = out[k]
    const empty = v === null || v === undefined || v === '' || (Array.isArray(v) && v.length === 0) || (typeof v === 'object' && !Array.isArray(v) && Object.keys(v).length === 0)
    if (empty) delete out[k]
  }
  return out
}

function FallbackEditor({ value, onChange, candidates }: { value: string[]; onChange: (v: string[]) => void; candidates: { id: string; label: string }[] }) {
  const label = (id: string) => candidates.find((c) => c.id === id)?.label ?? id
  const free = candidates.filter((c) => !value.includes(c.id))
  return (
    <div className="wb-set-fallback">
      {value.length === 0 && <div className="wb-small wb-subtle">None: sessions wait for the account, or fail with its own message.</div>}
      {value.map((id, i) => (
        <div key={id} className="wb-row">
          <span className="wb-subtle wb-small">{i + 1}.</span>
          <span className="wb-grow">{label(id)}</span>
          <IconButton icon={ArrowUp} label="Earlier" disabled={i === 0} onClick={() => onChange(moveItem(value, i, -1))} />
          <IconButton icon={ArrowDown} label="Later" disabled={i === value.length - 1} onClick={() => onChange(moveItem(value, i, 1))} />
          <IconButton icon={X} label="Remove" onClick={() => onChange(value.filter((x) => x !== id))} />
        </div>
      ))}
      {free.length > 0 && (
        <Select value="" onChange={(e) => e.target.value && onChange([...value, e.target.value])} aria-label="Add a fallback account">
          <option value="">Add an account…</option>
          {free.map((c) => (
            <option key={c.id} value={c.id}>
              {c.label}
            </option>
          ))}
        </Select>
      )}
    </div>
  )
}

const HOURS = [
  { label: '1 hour', ms: 3_600_000 },
  { label: '5 hours', ms: 5 * 3_600_000 },
  { label: '1 day', ms: 24 * 3_600_000 },
  { label: '7 days', ms: 7 * 24 * 3_600_000 },
]

function UsageControls({ id, usage }: { id: string; usage: AccountUsage | undefined }) {
  const [ms, setMs] = useState(HOURS[1].ms)
  const run = async (until: number | null) => {
    try {
      await setAccountLimit(id, until)
    } catch (e) {
      toastError(e, 'Could not change the account')
    }
  }
  return (
    <div className="wb-row" style={{ flexWrap: 'wrap' }}>
      {usage?.limited ? (
        <>
          <span className="wb-small">At its limit until {formatUntil(usage.limitedUntil ?? Date.now())}.</span>
          <Button size="small" onClick={() => void run(null)}>
            Mark usable
          </Button>
        </>
      ) : (
        <>
          <span className="wb-small wb-subtle">Usable.</span>
          <Select value={String(ms)} onChange={(e) => setMs(Number(e.target.value))} aria-label="For how long">
            {HOURS.map((h) => (
              <option key={h.ms} value={h.ms}>
                {h.label}
              </option>
            ))}
          </Select>
          <Button size="small" onClick={() => void run(Date.now() + ms)}>
            Mark at limit
          </Button>
        </>
      )}
    </div>
  )
}

function AccountEditor({
  id: editId,
  initial,
  providers,
  usage,
  onClose,
  onSave,
}: {
  id: string | null
  initial: ProviderSettings | null
  providers: Providers
  usage: AccountUsage | undefined
  onClose: () => void
  onSave: (id: string, config: ProviderSettings) => Promise<void>
}) {
  const editing = editId !== null
  const builtIn = editId !== null && BUILT_IN.includes(editId)
  const rows = accountRows(providers)
  const [local, setLocal] = useState(!!initial?.local)
  const [kind, setKind] = useState<AccountKind>((accountKind(initial?.kind ?? editId)?.kind ?? 'claude') as AccountKind)
  const k = accountKind(kind)!
  const servers = LOCAL_BY_KIND[kind]
  const [server, setServer] = useState(initial?.local?.server ?? servers[0] ?? 'ollama')
  const [url, setUrl] = useState(initial?.local?.url ?? '')
  const [model, setModel] = useState(initial?.model ?? '')
  const [found, setFound] = useState<{ models: string[]; error?: string } | null>(null)
  const [detecting, setDetecting] = useState(false)
  const [label, setLabel] = useState(initial?.label ?? '')
  const [id, setId] = useState(editId ?? '')
  const [idTouched, setIdTouched] = useState(editing)
  const [home, setHome] = useState(initial?.env?.[k.homeVar] ?? '')
  const [homeTouched, setHomeTouched] = useState(editing)
  const [enabled, setEnabled] = useState(initial?.enabled !== false)
  const [fallback, setFallback] = useState<string[]>(initial?.fallback ?? [])
  const [busy, setBusy] = useState(false)

  // Aider on a model server needs no keys file; every other CLI keeps its files in a folder of its own.
  const needsHome = !builtIn && !(local && kind === 'aider')
  const effectiveId = idTouched ? id : label.trim() ? suggestAccountId(kind, label) : local ? suggestLocalId(kind, server, model) : ''
  const effectiveHome = homeTouched ? home : effectiveId ? k.suggest(effectiveId.replace(`${kind}-`, '')) : ''
  const others = rows.filter((r) => r.id !== editId && r.kind?.kind === kind && r.home).map((r) => ({ id: r.id, home: r.home }))
  const idError = editing ? null : accountIdError(effectiveId, Object.keys(providers))
  const homeError = needsHome ? accountHomeError(kind, effectiveHome, others) : null
  const localError = local && !builtIn ? (localUrlError(server, url) ?? (model.trim() ? null : 'Enter the model the server runs')) : null
  const error = builtIn ? null : (idError ?? homeError ?? localError)

  const detect = async () => {
    setDetecting(true)
    try {
      const r = await probeLocalModels(server, url.trim())
      setFound(r)
      if (!model && r.models.length === 1) setModel(r.models[0])
    } catch (e) {
      setFound({ models: [], error: e instanceof Error ? e.message : String(e) })
    } finally {
      setDetecting(false)
    }
  }

  const pickLocal = (toLocal: boolean) => {
    const nextKind: AccountKind = toLocal && !LOCAL_BY_KIND[kind].length ? 'claude' : kind
    setLocal(toLocal)
    setKind(nextKind)
    setServer(LOCAL_BY_KIND[nextKind][0] ?? 'ollama')
    setFound(null)
  }

  const submit = async () => {
    if (error) return
    setBusy(true)
    try {
      // Everything else the section holds (args, effort, command…) is kept as it is.
      const env = { ...initial?.env }
      let config: ProviderSettings
      if (builtIn) {
        config = { ...initial, fallback }
      } else {
        if (needsHome) env[k.homeVar] = effectiveHome.trim()
        else delete env[k.homeVar]
        config = {
          ...initial,
          kind,
          label: label.trim() || (local ? `${model.trim()} on ${LOCAL_SERVERS[server].label}` : null),
          enabled: enabled ? null : false,
          fallback,
          env,
          model: local ? model.trim() : (initial?.model ?? null),
          local: local ? { server, url: url.trim() } : null,
        }
      }
      await onSave(builtIn ? editId : effectiveId, tidy(config))
      onClose()
    } catch {
      // reported by the caller
    } finally {
      setBusy(false)
    }
  }

  const self = builtIn ? editId : effectiveId
  const candidates = fallbackCandidates(self, providers)
  const title = builtIn ? `${accountName(editId, providers)} (built in)` : editing ? `Account “${editId}”` : local ? 'Add a local model' : 'Add an account'

  return (
    <Modal
      title={title}
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
      {builtIn ? (
        <div className="wb-small wb-muted">The login this CLI has outside Workbench. Only what it falls back to is set here.</div>
      ) : (
        <>
          {!editing && (
            <>
              <label className="wb-small wb-muted">Runs on</label>
              <Select value={local ? 'local' : 'login'} onChange={(e) => pickLocal(e.target.value === 'local')}>
                <option value="login">A subscription or API login</option>
                <option value="local">A model on this computer or my network</option>
              </Select>
            </>
          )}
          <div className="wb-small wb-muted">
            {local
              ? 'The CLI talks to a model server of your own instead of the vendor’s. Nothing of the vendor’s account is sent to it, and a session never falls back to the vendor if the server is missing.'
              : k.file
                ? 'Another set of API keys for Aider, such as a work and a personal one. Aider has no login: the keys live in a .env file of your own.'
                : 'Another login of the same CLI, for a second subscription. Its sessions, history and settings live in their own folder, next to the default one.'}
          </div>
          <label className="wb-small wb-muted">CLI</label>
          <Select
            value={kind}
            disabled={editing}
            onChange={(e) => {
              const next = e.target.value as AccountKind
              setKind(next)
              setServer(LOCAL_BY_KIND[next][0] ?? 'ollama')
              setFound(null)
            }}
          >
            {ACCOUNT_KINDS.filter((x) => !local || LOCAL_BY_KIND[x.kind].length > 0).map((x) => (
              <option key={x.kind} value={x.kind}>
                {x.label}
              </option>
            ))}
          </Select>
          {local && (
            <>
              <label className="wb-small wb-muted">Server</label>
              <Select
                value={server}
                onChange={(e) => {
                  setServer(e.target.value)
                  setFound(null)
                }}
              >
                {servers.map((s) => (
                  <option key={s} value={s}>
                    {LOCAL_SERVERS[s].label}
                  </option>
                ))}
              </Select>
              <label className="wb-small wb-muted">Address</label>
              <Input className="mono" value={url} onChange={(e) => setUrl(e.target.value)} placeholder={LOCAL_SERVERS[server].url || 'http://host:port'} />
              <div className="wb-small wb-muted">{LOCAL_SERVERS[server].hint}</div>
              <label className="wb-small wb-muted">Model</label>
              <div className="wb-row">
                <Input className="mono wb-grow" value={model} onChange={(e) => setModel(e.target.value)} placeholder="qwen3-coder:30b" list="wb-local-models" />
                <Button icon={RefreshCw} loading={detecting} disabled={!!localUrlError(server, url)} onClick={() => void detect()}>
                  Find models
                </Button>
              </div>
              <datalist id="wb-local-models">
                {found?.models.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
              {found && (
                <div className={found.error ? 'wb-field-error' : 'wb-small wb-muted'}>
                  {found.error ??
                    (found.models.length ? `${found.models.length} model${found.models.length === 1 ? '' : 's'} found: pick one from the list.` : 'The server answers but serves no model yet.')}
                </div>
              )}
            </>
          )}
          <label className="wb-small wb-muted">Label</label>
          <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder={local ? `${model.trim() || 'qwen3-coder'} on ${LOCAL_SERVERS[server].label}` : 'Work'} autoFocus={!editing && !local} />
          <div className="wb-small wb-muted">Shown in the new session picker and on sessions.{local ? '' : ' Empty: the name below.'}</div>
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
          {needsHome && (
            <>
              <label className="wb-small wb-muted">{k.file ? 'Keys file' : 'Folder'}</label>
              <Input
                className="mono"
                value={effectiveHome}
                onChange={(e) => {
                  setHome(e.target.value)
                  setHomeTouched(true)
                }}
                placeholder={k.suggest('work')}
              />
              <div className="wb-small wb-muted">
                Becomes <code>{k.homeVar}</code> for this account’s sessions.{' '}
                {local
                  ? 'Keeps this model’s sessions and history apart from your logins.'
                  : k.file
                    ? 'Create the file yourself, with KEY=value lines such as OPENAI_API_KEY, and keep it private: Workbench passes only its path and never reads it. Sessions refuse to start while it is missing. Aider keeps its chat history in each repository, shared by all accounts.'
                    : kind === 'gemini'
                      ? 'The folder that holds the .gemini folder: Gemini CLI creates it there and asks you to sign in the first time it runs.'
                      : 'The CLI creates it and asks you to sign in the first time it runs there.'}
              </div>
            </>
          )}
        </>
      )}
      <label className="wb-small wb-muted">When it is at its usage limit, use</label>
      <FallbackEditor value={fallback} onChange={setFallback} candidates={candidates} />
      {editing && (
        <>
          <label className="wb-small wb-muted">Usage</label>
          <UsageControls id={editId} usage={usage} />
        </>
      )}
      {editing && !builtIn && (
        <Checkbox checked={enabled} onChange={setEnabled}>
          Offer this account when starting a session
        </Checkbox>
      )}
      {error && (effectiveId || effectiveHome || local) && <div className="wb-field-error">{error}</div>}
    </Modal>
  )
}

function UsageCell({ usage }: { usage: AccountUsage | undefined }) {
  if (!usage || (usage.windows.length === 0 && !usage.limited)) return <span className="wb-subtle wb-small">No data yet</span>
  return (
    <div className="wb-usage">
      {usage.limited && (
        <Badge tone="danger" title={usage.reason ?? undefined}>
          At its limit until {formatUntil(usage.limitedUntil ?? Date.now())}
        </Badge>
      )}
      {usage.windows.map((w) => (
        <div key={w.name} className="wb-usage-row" title={w.resetsAt ? `${w.label}: resets ${formatUntil(w.resetsAt)}` : w.label}>
          <span className="wb-usage-label">{w.label}</span>
          <span className="wb-usage-track">
            <span className={`wb-usage-fill ${usageTone(w.usedPct)}`} style={{ width: `${Math.max(2, Math.min(100, w.usedPct))}%` }} />
          </span>
          <span className="wb-usage-pct">{Math.round(w.usedPct)}%</span>
        </div>
      ))}
    </div>
  )
}

const FAILOVER: { value: FailoverMode; label: string; hint: string }[] = [
  { value: 'new', label: 'New sessions use the next account', hint: 'A session you start skips an account that is at its limit and runs on the first account of its list that is not.' },
  { value: 'session', label: 'New sessions, and running ones continue', hint: 'A session that hits its limit also continues on the next account, as a new session that is told where the old one stopped.' },
  { value: 'off', label: 'Only show usage', hint: 'Nothing is decided for you.' },
]

/** Accounts of the agent CLIs: more than one login, models of your own, their usage, and what to fall back to. */
export function AccountsGroup() {
  const settings = useSettings()
  const usage = useAccountUsage()
  const [editor, setEditor] = useState<{ id: string | null; config: ProviderSettings | null } | null>(null)
  const providers: Providers = settings.data?.config.agents.providers ?? {}
  const rows = accountRows(providers)
  const mode: FailoverMode = (FAILOVER.find((f) => f.value === settings.data?.config.agents.failover)?.value ?? 'new') as FailoverMode

  const save = async (next: Providers, what: string) => {
    try {
      reportApply(await patchSettings({ agents: { providers: next } }, settings.data?.hash), what)
    } catch (e) {
      toastError(e, 'Could not save')
      throw e
    }
  }

  const remove = async (id: string, label: string) => {
    const dependents = Object.entries(providers)
      .filter(([x, c]) => x !== id && c.fallback?.includes(id))
      .map(([x]) => accountName(x, providers))
    const ok = await confirmDialog({
      title: `Remove “${label}”?`,
      message:
        (dependents.length ? `${dependents.join(', ')} fall${dependents.length === 1 ? 's' : ''} back to it; they will not any more. ` : '') +
        'Its sessions that are still open keep running, but cannot be restarted or resumed from Workbench. Its folder and login are left alone: add it again to get them back.',
      confirmLabel: 'Remove',
      danger: true,
    })
    if (!ok) return
    const next: Providers = {}
    for (const [x, c] of Object.entries(providers)) {
      if (x === id) continue
      next[x] = c.fallback?.includes(id) ? tidy({ ...c, fallback: c.fallback.filter((f) => f !== id) }) : c
    }
    await save(next, `${label} removed`).catch(() => {})
  }

  const setMode = async (value: FailoverMode) => {
    try {
      reportApply(await patchSettings({ agents: { failover: value } }, settings.data?.hash), 'Saved')
    } catch (e) {
      toastError(e, 'Could not save')
    }
  }

  if (!settings.data) return null
  const usageOf = (id: string) => usage.data?.usage[id]
  return (
    <>
      <Group
        title="Accounts"
        description="Run Claude Code, Codex, Kimi Code, Gemini CLI or Aider under more than one login, such as a work and a personal subscription, or on a model of your own. Each account keeps its login, history and settings in its own folder (Aider: its own keys file), and appears next to the CLI when you start a session. When an account reaches its usage limit, Workbench can move on to the next one."
        actions={
          <>
            <Button icon={Plus} onClick={() => setEditor({ id: null, config: null })}>
              Add account
            </Button>
            <Button icon={Plus} onClick={() => setEditor({ id: null, config: { kind: 'claude', local: { server: 'ollama', url: '' } } })}>
              Add local model
            </Button>
          </>
        }
        flush
      >
        {rows.length === 0 ? (
          <div className="wb-set-box">
            <EmptyState title="No accounts">Sessions use whichever login each CLI has outside Workbench.</EmptyState>
          </div>
        ) : (
          <div className="wb-set-box wb-acct-list">
            {rows.map((r) => {
              const name = accountName(r.id, providers)
              const l = r.config.local
              return (
                <div key={r.id} className="wb-acct">
                  <div className="wb-row">
                    <div className="wb-grow">
                      <span className="wb-acct-name">{name}</span>{' '}
                      {r.builtIn && <Badge>built in</Badge>} {l && <Badge tone="accent">local</Badge>} {r.config.enabled === false && <Badge>hidden</Badge>}{' '}
                      <span className="mono wb-subtle wb-small">{r.id}</span>
                    </div>
                    <IconButton icon={Pencil} label="Edit" onClick={() => setEditor({ id: r.id, config: r.config })} />
                    {!r.builtIn && <IconButton icon={Trash2} label="Remove" onClick={() => void remove(r.id, name)} />}
                  </div>
                  <div className="wb-small wb-muted wb-acct-line">
                    {r.builtIn ? (
                      'The CLI’s own login'
                    ) : l ? (
                      <>
                        {LOCAL_SERVERS[l.server]?.label ?? l.server} · <span className="mono">{r.config.model}</span> · <span className="mono">{l.url || LOCAL_SERVERS[l.server]?.url}</span>
                      </>
                    ) : (
                      <>
                        {r.kind?.file ? 'Keys in ' : 'Folder '}
                        <span className="mono">{r.home || 'not set'}</span>
                      </>
                    )}
                  </div>
                  {!l && <UsageCell usage={usageOf(r.id)} />}
                  {r.config.fallback?.length ? (
                    <div className="wb-small wb-acct-line">
                      <span className="wb-subtle">At its limit, use </span>
                      {r.config.fallback.map((f) => accountName(f, providers)).join(' → ')}
                    </div>
                  ) : null}
                </div>
              )
            })}
          </div>
        )}
      </Group>
      <Group
        title="Usage limits"
        description="Usage comes from what each CLI reports about itself: Claude Code’s status line and Codex’s session log. Workbench asks no vendor and reads no login."
      >
        <Row label="When an account is at its limit" hint={FAILOVER.find((f) => f.value === mode)?.hint}>
          <Select value={mode} onChange={(e) => void setMode(e.target.value as FailoverMode)}>
            {FAILOVER.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </Select>
        </Row>
      </Group>
      <div style={{ marginTop: 12 }}>
        <Note>
          To sign an account in, start a session with it: its CLI shows its own login the first time it runs in the new folder (Aider reads the keys in its file). Accounts are saved as{' '}
          <code>[agents.providers.&lt;name&gt;]</code> in <code>config.toml</code>, where model, effort and extra arguments can be set per account.
        </Note>
      </div>
      {editor && (
        <AccountEditor
          id={editor.id}
          initial={editor.config}
          providers={providers}
          usage={editor.id ? usageOf(editor.id) : undefined}
          onClose={() => setEditor(null)}
          onSave={(id, config) => {
            const label = accountName(id, { ...providers, [id]: config })
            return save({ ...providers, [id]: config }, editor.id ? `${label} saved` : `${label} added`).then(() => {
              if (!editor.id && config.local) toast('info', `Start a session with ${label} to try it`)
            })
          }}
        />
      )}
    </>
  )
}
