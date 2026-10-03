// "New session" composer: the agent CLI (provider), prompt, name, model, effort,
// permission mode, Remote Control (Claude), and the project's starter prompts as chips.

import { useEffect, useMemo, useRef, useState } from 'react'
import { CircleQuestionMark, Container, FileCode, Play, Plus, Sparkles, TriangleAlert } from 'lucide-react'
import { useProjects } from '@/api/queries'
import type { TerminalInfo } from '@/api/types'
import { confirmDialog, openPanel, openSettings } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Button, Checkbox, ErrorBox, IconButton, Input, Kbd, Select, TextArea } from '@/ui'
import { startAgent, useAgentDefaults, useContainerAgents, type AgentDefaults, type ProviderInfo } from './api'
import { displayLabel, initialProvider, limitNote, isDangerous, pickerProviders, presetOf, PROVIDER_CONFIG_EXAMPLE, stateNote } from './lib/providers'
import { ProviderIcon } from './parts'
import { useAgentsUi, type NewSessionPrefill } from './store'

const CLAUDE_MODELS = ['fable', 'opus', 'sonnet', 'haiku']

/**
 * Ask for notification permission once, from a user gesture, on remote devices (a phone):
 * there the platform slice shows browser notifications. The Workbench computer itself
 * gets the server's desktop notifications instead.
 */
export function requestNotificationPermission() {
  if (typeof Notification === 'undefined' || Notification.permission !== 'default') return
  if (!useUi.getState().prefs.notifications) return
  const h = location.hostname
  if (h === 'localhost' || h === '::1' || h === '[::1]' || h.startsWith('127.')) return
  void Notification.requestPermission().catch(() => {})
}

function openRawConfig() {
  openPanel({ kind: 'settings', id: 'settings', title: 'Settings', params: { section: 'raw' } })
}

/** The provider chips. Unavailable ones stay selectable to show how to install them. */
function ProviderPicker({ providers, value, onChange }: { providers: ProviderInfo[]; value: string | null; onChange: (id: string) => void }) {
  const byId = new Map(providers.map((p) => [p.id, p]))
  /** What a new session of `p` would start as instead, when it is at its limit. */
  const instead = (p: ProviderInfo): ProviderInfo | undefined => (p.fallback ?? []).map((f) => byId.get(f)).find((f) => f && f.enabled && f.available && !limitNote(f))
  return (
    <div className="wb-ag-providers">
      <div className="wb-ag-providers-group" role="radiogroup" aria-label="Agent">
        {providers.map((p) => {
          const limit = limitNote(p)
          const next = limit ? instead(p) : undefined
          return (
            <button
              key={p.id}
              role="radio"
              aria-checked={p.id === value}
              className={['wb-ag-provider', p.id === value && 'active', !p.available && 'unavailable', limit && 'limited'].filter(Boolean).join(' ')}
              title={
                !p.available
                  ? (p.reason ?? `${displayLabel(p)} is not available`)
                  : limit
                    ? `${displayLabel(p)} is ${limit}${next ? `: a new session starts on ${displayLabel(next)}` : ''}`
                    : p.local
                      ? `${displayLabel(p)} (${p.command}) on ${p.local.url || p.local.server}`
                      : p.api
                        ? `${displayLabel(p)} (${p.command}) through ${p.api.serviceLabel} with an API key`
                        : `${displayLabel(p)} (${p.command})`
              }
              onClick={() => onChange(p.id)}
            >
              <ProviderIcon kind={p.kind} />
              <span>{displayLabel(p)}</span>
              {!p.available && <span className="wb-ag-provider-note">not installed</span>}
              {p.available && limit && <span className="wb-ag-provider-note">{limit}</span>}
              {p.available && !limit && p.local && <span className="wb-ag-provider-note">local</span>}
              {p.available && !limit && p.api && <span className="wb-ag-provider-note">API</span>}
            </button>
          )
        })}
      </div>
      <button
        className="wb-ag-provider add"
        title="Add or manage accounts: more than one login of Claude Code, Codex, Kimi Code, Gemini CLI or Aider"
        onClick={() => openSettings('agents')}
      >
        <Plus size={12} />
        <span>Account</span>
      </button>
    </div>
  )
}

/** How providers are set up (the composer's "?"). */
function ProvidersHelp({ d }: { d: AgentDefaults }) {
  const caps = (p: ProviderInfo) =>
    [
      p.supports.resume && 'resume',
      p.supports.fork && 'fork',
      p.supports.mcp && 'Workbench MCP',
      p.supports.answerPermissions && d.answerPermissions && 'permissions answered here',
      p.stateSource === 'activity' ? 'state estimated from output' : 'exact state',
    ]
      .filter(Boolean)
      .join(' · ')
  return (
    <div className="wb-ag-help">
      <div>
        Workbench runs agent CLIs in its terminals. Claude Code, Codex, Kimi Code, Gemini CLI and Aider are built in; any other CLI can be added. They are
        configured in <code>config.toml</code> under <code>[agents.providers.&lt;name&gt;]</code> (the <code>[agents]</code> section keeps the Claude Code
        defaults). A second login of a CLI (a work and a personal subscription) is an account: add it with <b>Account</b> or in Settings → Agents.
      </div>
      {d.answerPermissions && (
        <div className="wb-muted">
          Claude Code&apos;s permission prompts can be answered from Workbench (the session&apos;s card and tab, a toast, the phone) for{' '}
          {Math.round((d.permissionWait ?? 600) / 60)} minutes; its own prompt in the terminal stays usable, and the first answer wins.
        </div>
      )}
      <ul className="wb-ag-help-list">
        {d.providers.map((p) => (
          <li key={p.id}>
            <ProviderIcon kind={p.kind} size={12} />
            <b>{displayLabel(p)}</b>
            <code title={p.command}>{p.command.split('/').pop()}</code>
            <span className="wb-muted">— {p.enabled ? caps(p) : 'disabled'}</span>
          </li>
        ))}
      </ul>
      {d.providerWarnings.map((w) => (
        <div key={w} className="wb-warning wb-small">
          {w}
        </div>
      ))}
      <pre className="wb-ag-help-code">{PROVIDER_CONFIG_EXAMPLE}</pre>
      <div>
        <Button size="small" icon={FileCode} onClick={openRawConfig}>
          Open Raw config
        </Button>
      </div>
    </div>
  )
}

export function NewSessionForm({
  projectId,
  prefill,
  onStarted,
  autoFocus,
  focusToken,
  compact,
}: {
  projectId: string | null
  prefill?: NewSessionPrefill
  onStarted?: (t: TerminalInfo) => void
  autoFocus?: boolean
  /** Put the caret into the prompt each time this changes (the agents column's "+"). */
  focusToken?: number
  /** Phone: prompt + start only; options folded. */
  compact?: boolean
}) {
  const defaults = useAgentDefaults(projectId)
  const d = defaults.data
  const lastProvider = useAgentsUi((s) => s.lastProvider)
  const setLastProvider = useAgentsUi((s) => s.setLastProvider)
  const [chosen, setChosen] = useState<string | null>(prefill?.provider ?? null)
  const [prompt, setPrompt] = useState(prefill?.prompt ?? '')
  const [name, setName] = useState(prefill?.name ?? '')
  const [model, setModel] = useState('')
  const [effort, setEffort] = useState('')
  const [mode, setMode] = useState('')
  const [remote, setRemote] = useState<boolean | null>(null)
  const [showOptions, setShowOptions] = useState(!compact)
  const [help, setHelp] = useState(false)
  const [busy, setBusy] = useState(false)
  const [inside, setInside] = useState<boolean | null>(null)
  const ref = useRef<HTMLTextAreaElement>(null)
  // The project's dev container: when it runs, a session can run inside it.
  const projects = useProjects()
  const dc = projects.data?.find((x) => x.id === projectId)?.devcontainer
  const dcRunning = dc?.state === 'running'
  const containerAgents = useContainerAgents(projectId, dcRunning)

  useEffect(() => {
    if (autoFocus) ref.current?.focus()
  }, [autoFocus])
  const lastFocus = useRef(focusToken)
  useEffect(() => {
    if (focusToken === lastFocus.current) return
    lastFocus.current = focusToken
    ref.current?.focus()
  }, [focusToken])

  const providers = useMemo(() => pickerProviders(d?.providers ?? []), [d])
  const providerId = chosen && providers.some((p) => p.id === chosen) ? chosen : d ? initialProvider(d.providers, lastProvider, d.defaultProvider) : null
  const p = providers.find((x) => x.id === providerId)
  const cliName = p?.command.split('/').pop() ?? ''
  const insidePath = dcRunning ? containerAgents.data?.agents?.[cliName] : undefined
  const canInside = dcRunning && !!insidePath
  // Default: inside when the project runs its terminals there and the CLI is installed.
  const runInside = canInside && (inside ?? !!dc?.inContainer)

  if (!projectId) return <div className="wb-muted wb-small wb-pad">Open a project to start a session.</div>

  const pick = (id: string) => {
    if (id === providerId) return
    setChosen(id)
    // Model, effort and permission values belong to one provider.
    setModel('')
    setEffort('')
    setMode('')
  }

  const preset = presetOf(p, mode || p?.defaults.permissionMode)
  const dangerous = isDangerous(p, mode)
  const label = p ? (p.kind === 'claude' && p.id === 'claude' ? 'Claude' : displayLabel(p)) : 'the agent'

  const start = async (text: string, n?: string) => {
    if (busy || !p || (!p.available && !runInside)) return
    if (dangerous) {
      const ok = await confirmDialog({
        title: `Start ${displayLabel(p)} without approvals?`,
        message: `${preset?.label}: ${preset?.description}. It can change files and run commands without asking.`,
        confirmLabel: 'Start anyway',
        danger: true,
      })
      if (!ok) return
    }
    setBusy(true)
    requestNotificationPermission()
    setLastProvider(p.id)
    const t = await startAgent({
      projectId,
      provider: p.id,
      prompt: text.trim() || undefined,
      name: (n ?? name).trim() || undefined,
      model: model.trim() || undefined,
      effort: effort || undefined,
      permissionMode: mode || undefined,
      remoteControl: p.supports.remoteControl ? (remote ?? undefined) : undefined,
      inContainer: runInside || undefined,
    })
    setBusy(false)
    if (t) {
      setPrompt('')
      setName('')
      onStarted?.(t)
    }
  }

  const claudeModels = p?.defaults.model && !CLAUDE_MODELS.includes(p.defaults.model) ? [...CLAUDE_MODELS, p.defaults.model] : CLAUDE_MODELS
  const note = p && showOptions ? stateNote(p) : null
  return (
    <div className={compact ? 'wb-ag-composer compact' : 'wb-ag-composer'}>
      {defaults.error && <ErrorBox error={defaults.error} onRetry={() => void defaults.refetch()} />}
      {d && (
        <div className="wb-ag-composer-head">
          <ProviderPicker providers={providers} value={providerId} onChange={pick} />
          {d.providerWarnings.length > 0 ? (
            // Settings config.toml has that Workbench ignores (a typo in default_provider, an
            // effort a CLI does not take) are shown here, not only inside the help.
            <IconButton
              icon={TriangleAlert}
              size="small"
              className="warn"
              label={`${d.providerWarnings.length} agent setting${d.providerWarnings.length > 1 ? 's' : ''} in config.toml ignored — show why`}
              active={help}
              onClick={() => setHelp(!help)}
            />
          ) : (
            <IconButton icon={CircleQuestionMark} size="small" label="Agent CLIs and how to configure them" active={help} onClick={() => setHelp(!help)} />
          )}
        </div>
      )}
      {d && help && <ProvidersHelp d={d} />}
      {p && !p.available && !runInside && (
        <div className="wb-error setup">
          {p.reason}.{' '}
          {p.installHint && (
            <>
              Install it with <code>{p.installHint}</code>, or set{' '}
            </>
          )}
          {!p.installHint && 'Set '}
          <code>{p.id === 'claude' ? '[agents].command' : `[agents.providers.${p.id}].command`}</code> in config.toml.
        </div>
      )}
      <TextArea
        ref={ref}
        rows={3}
        value={prompt}
        placeholder={`What should ${label} do? Leave empty to start an interactive session.`}
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
            e.preventDefault()
            void start(prompt)
          }
        }}
      />
      {showOptions && p && (
        <div className="wb-ag-composer-row">
          <Input small placeholder="Name (optional)" value={name} onChange={(e) => setName(e.target.value)} style={{ width: 150 }} />
          {p.kind === 'claude' ? (
            <Select value={model} onChange={(e) => setModel(e.target.value)} title="Model">
              <option value="">Model: {p.defaults.model ?? 'default'}</option>
              {claudeModels.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Select>
          ) : (
            p.supports.model && (
              <Input small placeholder={`Model: ${p.defaults.model ?? 'default'}`} value={model} onChange={(e) => setModel(e.target.value)} style={{ width: 150 }} title="Model" />
            )
          )}
          {p.efforts.length > 0 && (
            <Select value={effort} onChange={(e) => setEffort(e.target.value)} title="Reasoning effort">
              <option value="">Effort: {p.defaults.effort ?? 'default'}</option>
              {p.efforts.map((x) => (
                <option key={x} value={x}>
                  {x}
                </option>
              ))}
            </Select>
          )}
          {p.permissionModes.length > 0 && (
            <Select value={mode} onChange={(e) => setMode(e.target.value)} title={preset?.description ?? 'Permission mode'} className={dangerous ? 'danger' : undefined}>
              <option value="">Permissions: {presetOf(p, p.defaults.permissionMode)?.label ?? 'default'}</option>
              {p.permissionModes.map((x) => (
                <option key={x.id} value={x.id}>
                  {x.label}
                  {x.dangerous ? ' ⚠' : ''}
                </option>
              ))}
            </Select>
          )}
          {p.supports.remoteControl && (
            <Checkbox checked={remote ?? d?.remoteControl ?? false} onChange={setRemote}>
              Remote Control
            </Checkbox>
          )}
        </div>
      )}
      {dangerous && preset && (
        <div className="wb-ag-danger">
          <TriangleAlert size={13} />
          <span>
            {preset.label}: {preset.description}. You will be asked to confirm.
          </span>
        </div>
      )}
      {note && <div className="wb-ag-note">{note}</div>}
      <div className="wb-ag-composer-row">
        {!!d?.starters.length && (
          <div className="wb-ag-starters">
            {d.starters.map((s) => (
              <button key={s.name} className="wb-ag-starter" title={s.prompt} disabled={busy || (!p?.available && !runInside)} onClick={() => void start(s.prompt, s.name)}>
                <Sparkles size={12} />
                {s.name}
              </button>
            ))}
          </div>
        )}
        {!showOptions && (
          <button className="wb-ag-linkish" onClick={() => setShowOptions(true)}>
            Options…
          </button>
        )}
        <span style={{ flex: 1 }} />
        {dcRunning && p && (
          <span
            className="wb-ag-inside"
            title={
              canInside
                ? `${cliName} runs in the dev container (${insidePath}); hooks and MCP reach Workbench through its bridge listener`
                : `${cliName} is not installed in the dev container${containerAgents.isLoading ? ' (checking…)' : ''}`
            }
          >
            <Checkbox checked={runInside} disabled={!canInside} onChange={setInside}>
              <Container size={12} /> Run in dev container
            </Checkbox>
          </span>
        )}
        {!compact && (
          <span className="wb-subtle wb-xs">
            <Kbd>Ctrl+Enter</Kbd>
          </span>
        )}
        <Button variant="primary" icon={Play} loading={busy} disabled={!p?.available && !runInside} onClick={() => void start(prompt)}>
          Start session
        </Button>
      </div>
    </div>
  )
}
