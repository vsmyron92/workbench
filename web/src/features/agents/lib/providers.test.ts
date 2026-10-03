import { describe, expect, it } from 'vitest'
import type { AgentInfo, AgentProvider } from '@/api/types'
import type { ProviderInfo } from '../api'
import {
  dialogSeenOnScreen,
  displayLabel,
  historyProviders,
  initialProvider,
  isDangerous,
  pickerProviders,
  presetOf,
  providerIdOf,
  providerKindOf,
  providerLabel,
  reportsAnswers,
  resumes,
  stateNote,
} from './providers'

function provider(id: string, kind: AgentProvider, over: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id,
    kind,
    label: id,
    command: id,
    enabled: true,
    available: true,
    reason: null,
    installHint: null,
    stateSource: kind === 'claude' ? 'hooks' : kind === 'codex' ? 'rollout' : 'activity',
    initialPrompt: kind === 'claude' || kind === 'codex' ? 'argv' : 'paste',
    supports: { resume: kind !== 'custom', fork: kind === 'claude' || kind === 'codex', model: kind !== 'custom', mcp: false, remoteControl: kind === 'claude', addDirs: true, history: kind !== 'custom' },
    efforts: [],
    permissionModes: [],
    defaults: { model: null, effort: null, permissionMode: null },
    ...over,
  }
}

const withAgent = (a: Partial<AgentInfo> | null) => ({ agent: a === null ? null : ({ provider: 'claude', providerId: null, ...a } as AgentInfo) })

describe('providers', () => {
  const claude = provider('claude', 'claude', { label: 'Claude Code' })
  const codex = provider('codex', 'codex', {
    label: 'Codex',
    permissionModes: [
      { id: 'workspace-write', label: 'Workspace write', description: 'd', dangerous: false },
      { id: 'bypass', label: 'Bypass', description: 'd', dangerous: true },
    ],
  })
  const kimi = provider('kimi', 'kimi', { label: 'Kimi Code', available: false, reason: 'not found' })
  const aider = provider('aider', 'custom', { label: 'Aider' })
  const off = provider('off', 'custom', { enabled: false })
  const all = [claude, codex, kimi, aider, off]

  it('names the provider of a session', () => {
    expect(providerIdOf(withAgent({ providerId: null }))).toBe('claude')
    expect(providerIdOf(withAgent({ provider: 'codex', providerId: 'codex-work' }))).toBe('codex-work')
    expect(providerKindOf(withAgent(null))).toBe('claude')
    expect(providerLabel(withAgent({ providerId: 'claude' }), all)).toBe('Claude')
    expect(providerLabel(withAgent({ provider: 'codex', providerId: 'codex' }), all)).toBe('Codex')
    expect(providerLabel(withAgent({ provider: 'custom', providerId: 'aider' }), all)).toBe('Aider')
    // Without the provider list (or a provider removed from config): by kind or id.
    expect(providerLabel(withAgent({ provider: 'kimi', providerId: 'kimi' }))).toBe('Kimi')
    expect(providerLabel(withAgent({ provider: 'custom', providerId: 'gone' }))).toBe('gone')
  })

  it('starts the composer with a provider that can start', () => {
    expect(initialProvider(all, 'aider', 'claude')).toBe('aider')
    expect(initialProvider(all, 'kimi', 'codex')).toBe('codex') // kimi is not installed
    expect(initialProvider(all, null, 'nope')).toBe('claude')
    expect(initialProvider([kimi], null, 'claude')).toBe('kimi') // nothing available: still show one
    expect(initialProvider([], null, 'claude')).toBeNull()
  })

  it('orders the picker and the history tabs', () => {
    expect(pickerProviders(all).map((p) => p.id)).toEqual(['claude', 'codex', 'aider', 'kimi'])
    expect(historyProviders(all).map((p) => p.id)).toEqual(['claude', 'codex', 'kimi'])
  })

  it('knows dangerous permission modes', () => {
    expect(isDangerous(codex, 'bypass')).toBe(true)
    expect(isDangerous(codex, 'workspace-write')).toBe(false)
    expect(isDangerous(codex, '')).toBe(false)
    expect(isDangerous(undefined, 'bypass')).toBe(false)
    expect(presetOf(codex, 'bypass')?.label).toBe('Bypass')
  })

  it('explains estimated state', () => {
    expect(stateNote(claude)).toBeNull()
    expect(stateNote(codex)).toContain('session log')
    expect(stateNote(codex)).toContain('approval prompts are recognized')
    expect(stateNote(aider)).toContain('estimated from terminal output')
    expect(stateNote(aider)).toContain('always starts a new')
    expect(stateNote(provider('kimi', 'kimi'))).toContain('dialogs are recognized')
    expect(stateNote(provider('gemini', 'gemini'))).toContain('dialogs are recognized')
    expect(stateNote(provider('aider', 'aider'))).toContain('restores the repository')
  })

  it('tells hooks from screen heuristics', () => {
    expect(dialogSeenOnScreen(withAgent({ provider: 'codex', state: 'needs_permission' }))).toBe(true)
    expect(dialogSeenOnScreen(withAgent({ provider: 'gemini', state: 'needs_input' }))).toBe(true)
    expect(dialogSeenOnScreen(withAgent({ provider: 'claude', state: 'needs_permission' }))).toBe(false)
    expect(dialogSeenOnScreen(withAgent({ provider: 'aider', state: 'working' }))).toBe(false)
    expect(reportsAnswers('codex') && reportsAnswers('claude')).toBe(true)
    expect(reportsAnswers('gemini') || reportsAnswers('aider') || reportsAnswers('kimi')).toBe(false)
    expect(providerLabel(withAgent({ provider: 'gemini', providerId: 'gemini' }))).toBe('Gemini')
    expect(resumes(withAgent({ provider: 'aider' })) && !resumes(withAgent({ provider: 'custom' }))).toBe(true)
  })
})

describe('displayLabel', () => {
  it('puts the CLI before the label of a second account', () => {
    const work = provider('claude-work', 'claude', { label: 'Work', home: '~/.claude-work' })
    expect(displayLabel(work)).toBe('Claude · Work')
    expect(displayLabel(provider('codex-team', 'codex', { label: 'Team', home: '~/.codex-team' }))).toBe('Codex · Team')
    // A label that names the CLI already is kept, as are the presets and CLIs without a folder.
    expect(displayLabel(provider('claude-work', 'claude', { label: 'Claude (work)', home: '~/.claude-work' }))).toBe('Claude (work)')
    expect(displayLabel(provider('codex', 'codex', { label: 'Codex', home: '~/.codex-x' }))).toBe('Codex')
    expect(displayLabel(provider('claude-work', 'claude', { label: 'Work' }))).toBe('Work')
    expect(displayLabel(provider('opencode', 'custom', { label: 'OpenCode' }))).toBe('OpenCode')
  })

  it('names the account on its sessions', () => {
    const list = [provider('claude', 'claude', { label: 'Claude Code' }), provider('claude-work', 'claude', { label: 'Work', home: '~/.claude-work' })]
    expect(providerLabel(withAgent({ provider: 'claude', providerId: 'claude-work' }), list)).toBe('Claude · Work')
    expect(providerLabel(withAgent({ provider: 'claude', providerId: null }), list)).toBe('Claude')
  })
})
