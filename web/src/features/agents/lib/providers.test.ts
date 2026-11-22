import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { qk } from '@/api/queries'
import type { AgentInfo, AgentProvider, ProjectSummary } from '@/api/types'
import { agentKeys, type AgentDefaults, type ProviderInfo } from '../api'
import { AVATAR_HELP, AVATAR_LABEL, avatarDrive, AvatarHelp, AvatarOption, avatarWanted, NewSessionForm } from '../NewSession'
import { useAgentsUi } from '../store'
import {
  dialogSeenOnScreen,
  carryKind,
  carryText,
  displayLabel,
  movedToast,
  limitEnds,
  limitNote,
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
    avatarVia: kind === 'claude' || kind === 'codex' ? 'tools' : 'command',
    // In a dev container only Claude Code has Workbench's tools (the server's `avatar::via_for`).
    avatarViaContainer: kind === 'claude' ? 'tools' : 'command',
    supports: { resume: kind !== 'custom', fork: kind === 'claude' || kind === 'codex', model: kind !== 'custom', mcp: false, remoteControl: kind === 'claude', restricted: kind === 'claude', ultracode: kind === 'claude', avatar: true, addDirs: true, history: kind !== 'custom' },
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

describe('limitNote', () => {
  const now = new Date(2026, 9, 3, 14, 0).getTime()
  const usage = (limited: boolean, until: number | null) => ({ usage: { limited, limitedUntil: until, reason: null, windows: [] } })
  it('says when an account at its limit is usable again', () => {
    expect(limitNote(usage(true, new Date(2026, 9, 3, 15, 45).getTime()), now)).toMatch(/^at limit until 3:45\s?PM$/i)
    expect(limitNote(usage(true, new Date(2026, 9, 5, 0, 0).getTime()), now)).toMatch(/^at limit until \w{3} 12:00\s?AM$/i)
  })
  it('says nothing about an account that is usable, or whose limit has passed', () => {
    expect(limitNote(usage(false, null), now)).toBeNull()
    expect(limitNote(usage(true, now - 1), now)).toBeNull()
    expect(limitNote({}, now)).toBeNull()
    expect(limitEnds(new Date(2026, 9, 20, 9, 0).getTime(), now)).toMatch(/20/)
  })
})

describe('carrying a conversation', () => {
  it('resumes it for the same CLI and writes it out for another', () => {
    expect(carryKind('claude', 'claude')).toBe('resume')
    expect(carryKind('codex', 'codex')).toBe('resume')
    expect(carryKind('claude', 'codex')).toBe('digest')
    expect(carryKind('codex', 'aider')).toBe('digest')
    expect(carryKind('gemini', 'claude')).toBe('notes')
    expect(carryKind('aider', 'aider')).toBe('notes')
    expect(carryKind('claude', 'claude', 'notes')).toBe('notes')
  })

  it('says what goes where', () => {
    expect(carryText('resume', { kind: 'claude', local: null }, 'claude')).toBe('The conversation itself continues there.')
    expect(carryText('resume', { kind: 'claude', local: { server: 'ollama', url: '' } }, 'claude')).toMatch(/stays on your network/)
    expect(carryText('digest', { kind: 'codex', local: null }, 'claude')).toMatch(/sent to that CLI’s service/)
    expect(carryText('digest', { kind: 'aider', local: { server: 'ollama', url: '' } }, 'claude')).toMatch(/stays on your network/)
    expect(carryText('notes', { kind: 'codex', local: null }, 'claude')).toMatch(/short note/)
    // A hosted API is a third party, also for the same CLI.
    const ds = { kind: 'claude' as const, local: null, api: { service: 'deepseek', serviceLabel: 'DeepSeek', url: '', key: 'k' } }
    expect(carryText('resume', ds, 'claude')).toBe('The conversation itself continues there. It is sent to DeepSeek.')
    expect(carryText('digest', { ...ds, kind: 'aider' as const }, 'claude')).toMatch(/sent to DeepSeek/)
  })

  it('titles the toast by how it was carried', () => {
    expect(movedToast('resume', 'Claude · Work')).toEqual({ title: 'Continued on Claude · Work', detail: 'The same conversation, resumed.' })
    expect(movedToast('digest', 'Codex').detail).toMatch(/as text/)
    expect(movedToast('notes', 'Codex').title).toBe('Started on Codex')
    expect(movedToast(undefined, 'X').title).toBe('Started on X')
  })
})

// The composer's Avatar control. Its pieces are rendered on the server from props; the
// whole composer is rendered too, where the control reads the store's server snapshot (its
// initial state: off), which is enough to see where it is and for which CLI.
describe('the composer’s Avatar control', () => {
  const claude = provider('claude', 'claude', { label: 'Claude Code' })
  const kimi = provider('kimi', 'kimi', { label: 'Kimi Code' })
  /** A CLI as the server lists it while avatars are switched off in Settings. */
  const without = (p: ProviderInfo): ProviderInfo => ({ ...p, supports: { ...p.supports, avatar: false } })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetModules()
  })

  it('is a check box labelled Avatar, built like the other session options, with its tooltip', () => {
    const option = (on: boolean) => renderToStaticMarkup(createElement(AvatarOption, { on, onChange: () => {} }))
    expect(AVATAR_LABEL).toBe('Avatar: this session gets a live avatar')
    expect(option(false)).toBe(`<span title="${AVATAR_LABEL}"><label class="wb-checkbox"><input type="checkbox"/>Avatar</label></span>`)
    expect(option(true)).toBe(`<span title="${AVATAR_LABEL}"><label class="wb-checkbox"><input type="checkbox" checked=""/>Avatar</label></span>`)
    // No icon button any more: a smiley without a word read as an emoji picker.
    for (const html of [option(false), option(true)]) {
      expect(html).not.toContain('wb-ag-avatar')
      expect(html).not.toContain('<svg')
      expect(html).not.toContain('disabled')
    }
  })

  it('passes on what the box says when it is ticked or cleared', () => {
    const turned: boolean[] = []
    const box = (on: boolean) => ((AvatarOption({ on, onChange: (v) => turned.push(v) }).props as { children: { props: { checked: boolean; onChange: (v: boolean) => void } } }).children.props)
    expect(box(false).checked).toBe(false)
    expect(box(true).checked).toBe(true)
    box(false).onChange(true)
    box(true).onChange(false)
    expect(turned).toEqual([true, false])
  })

  it('asks for an avatar only while it is on and shown', () => {
    expect(avatarWanted(claude, true)).toBe(true)
    expect(avatarWanted(kimi, true)).toBe(true)
    expect(avatarWanted(claude, false)).toBe(false)
    // Remembered from before avatars were switched off, or no CLI chosen yet: nothing is asked for unseen.
    expect(avatarWanted(without(claude), true)).toBe(false)
    expect(avatarWanted(undefined, true)).toBe(false)
    // A server that does not list the capability, and a stored value that is no boolean.
    const older = { supports: { ...claude.supports, avatar: undefined } } as unknown as ProviderInfo
    expect(avatarWanted(older, true)).toBe(false)
    for (const stored of ['true', 1, null, undefined, {}]) expect(avatarWanted(claude, stored)).toBe(false)
  })

  it('says what an avatar is, and how a CLI without Workbench tools drives it', () => {
    const note = (p: ProviderInfo) => renderToStaticMarkup(createElement(AvatarHelp, { p }))
    for (const phrase of ['small animated character', 'draws its own look', 'aloud or as captions', 'click, typing or voice', 'half a minute', 'few thousand tokens']) expect(AVATAR_HELP).toContain(phrase)
    expect(note(claude)).toBe(`<div class="wb-ag-note">${AVATAR_HELP}</div>`)
    expect(note(kimi)).toBe(
      `<div class="wb-ag-note">${AVATAR_HELP} Kimi Code has no Workbench tools: it drives its avatar with the <code>workbench avatar</code> command and may ask you before running it, unless you allow that command.</div>`,
    )
    // A second account is named as the picker names it.
    expect(note(provider('gemini-work', 'gemini', { label: 'Work', home: '~/.gemini-work' }))).toContain(' Gemini · Work has no Workbench tools:')
    // A server that does not say how: the help alone.
    expect(note({ ...kimi, avatarVia: undefined } as unknown as ProviderInfo)).toBe(`<div class="wb-ag-note">${AVATAR_HELP}</div>`)
  })

  const codex = provider('codex', 'codex', { label: 'Codex' })
  const HOST = (label: string) => ` ${label} has no Workbench tools: it drives its avatar with the <code>workbench avatar</code> command and may ask you before running it, unless you allow that command.`
  const CONTAINER = (label: string) => ` In a dev container ${label} has no Workbench tools: it drives its avatar with <code>curl</code> and may ask you before running it, unless you allow that command.`

  it('knows what drives the avatar where the session will run: the host or the dev container', () => {
    // Claude Code has Workbench's tools everywhere; Codex only on the host; the others nowhere.
    expect([avatarDrive(claude, false), avatarDrive(claude, true)]).toEqual([null, null])
    expect([avatarDrive(codex, false), avatarDrive(codex, true)]).toEqual([null, 'curl'])
    expect([avatarDrive(kimi, false), avatarDrive(kimi, true)]).toEqual(['command', 'curl'])
    // Each answer comes from its own field: the host's says nothing about the container, and the other way round.
    expect(avatarDrive({ avatarVia: 'command', avatarViaContainer: 'tools' }, true)).toBeNull()
    expect(avatarDrive({ avatarVia: 'command', avatarViaContainer: 'tools' }, false)).toBe('command')
    expect(avatarDrive({ avatarVia: 'tools', avatarViaContainer: 'command' }, false)).toBeNull()
    // A server that does not say, or says something else: no sentence, never a guess.
    for (const via of [undefined, null, '', 'curl', 'COMMAND', true]) {
      const p = { avatarVia: via, avatarViaContainer: via } as unknown as ProviderInfo
      expect([avatarDrive(p, false), avatarDrive(p, true)]).toEqual([null, null])
    }
  })

  it('says the sentence of the place the form is set to, for each CLI', () => {
    const note = (p: ProviderInfo, inContainer?: boolean) => renderToStaticMarkup(createElement(AvatarHelp, { p, inContainer }))
    const plain = `<div class="wb-ag-note">${AVATAR_HELP}</div>`
    const withSentence = (sentence: string) => `<div class="wb-ag-note">${AVATAR_HELP}${sentence}</div>`
    // On the host (and when the form says nothing about a container).
    expect(note(claude, false)).toBe(plain)
    expect(note(codex, false)).toBe(plain)
    expect(note(kimi, false)).toBe(withSentence(HOST('Kimi Code')))
    expect(note(kimi)).toBe(note(kimi, false))
    // Run in dev container ticked: Codex loses its tools there, the command sentence gives way to the curl one.
    expect(note(claude, true)).toBe(plain)
    expect(note(codex, true)).toBe(withSentence(CONTAINER('Codex')))
    expect(note(kimi, true)).toBe(withSentence(CONTAINER('Kimi Code')))
    // Never both sentences, and an account is named as the picker names it.
    for (const html of [note(kimi, true), note(codex, true)]) expect(html).not.toContain('workbench avatar')
    expect(note(kimi, false)).not.toContain('curl')
    expect(note(provider('gemini-work', 'gemini', { label: 'Work', home: '~/.gemini-work' }), true)).toContain(' In a dev container Gemini · Work has no Workbench tools:')
    // A server that does not say what holds in a container: the help alone there.
    expect(note({ ...kimi, avatarViaContainer: undefined } as unknown as ProviderInfo, true)).toBe(plain)
    // The contract's wording, letter for letter (10.8 and D4), with the command names as code.
    const text = (html: string) => html.replace(/<\/?code>/g, '`').replace(/<[^>]+>/g, '')
    expect(text(note(kimi, false))).toBe(`${AVATAR_HELP} Kimi Code has no Workbench tools: it drives its avatar with the \`workbench avatar\` command and may ask you before running it, unless you allow that command.`)
    expect(text(note(codex, true))).toBe(`${AVATAR_HELP} In a dev container Codex has no Workbench tools: it drives its avatar with \`curl\` and may ask you before running it, unless you allow that command.`)
  })

  /** What a project's dev container is to the composer: its state, whether sessions run in it by default, and the CLIs installed in it. */
  interface Dev {
    state: 'running' | 'stopped'
    inContainer: boolean
    agents: Record<string, string | null>
  }

  /** The whole composer for a project whose CLIs are `providers` (the first one is chosen). `dev`: the project's dev container. */
  function composer(providers: ProviderInfo[], compact: boolean, dev?: Dev): string {
    const d: AgentDefaults = {
      command: 'claude',
      commandFound: true,
      model: null,
      effort: null,
      permissionMode: null,
      remoteControl: false,
      restoreOnStart: true,
      statusline: true,
      answerPermissions: true,
      permissionWait: 600,
      addDirs: [],
      starters: [],
      efforts: [],
      permissionModes: [],
      providers,
      defaultProvider: providers[0].id,
      providerWarnings: [],
    }
    const qc = new QueryClient()
    qc.setQueryData(agentKeys.defaults('shop'), d)
    if (dev) {
      qc.setQueryData(qk.projects, [{ id: 'shop', name: 'shop', devcontainer: { configs: [], state: dev.state, inContainer: dev.inContainer } } as unknown as ProjectSummary])
      qc.setQueryData(['agents', 'container', 'shop'], { agents: dev.agents, inContainer: dev.inContainer })
    }
    // The composer's microphone looks at `window` for speech recognition: this browser has it.
    vi.stubGlobal('window', { SpeechRecognition: class {} })
    return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, createElement(NewSessionForm, { projectId: 'shop', compact })))
  }
  /** The Avatar option in a rendered composer: its box and its label, or null. */
  const control = (html: string) => /<span title="Avatar: this session gets a live avatar"><label class="wb-checkbox"><input type="checkbox"[^>]*\/>Avatar<\/label><\/span>/.exec(html)?.[0] ?? null
  /** The option before the Avatar box, as the composer renders it. */
  const option = (label: string) => new RegExp(`<label class="wb-checkbox"><input type="checkbox"[^>]*/>${label}</label>`)

  it('is among the session options, after Ultracode, and the phone shows it where it shows them', () => {
    const desktop = composer([claude], false)
    const box = control(desktop)
    expect(box).not.toBeNull()
    expect(box).not.toContain('checked')
    // In the row of the other options (Name, Remote Control, Restricted, Ultracode), after Ultracode.
    const row = /<div class="wb-ag-composer-row">(?:(?!<div class="wb-ag-composer-row">).)*?Ultracode.*?<\/div>/.exec(desktop)?.[0] ?? ''
    expect(row).toContain('Name (optional)')
    expect(row).toContain(box)
    const at = (re: RegExp | string) => (typeof re === 'string' ? desktop.indexOf(re) : (re.exec(desktop)?.index ?? -1))
    for (const before of ['Remote Control', 'Restricted', 'Ultracode']) expect(at(option(before)), before).toBeGreaterThan(-1)
    expect(at(option('Ultracode'))).toBeLessThan(at(box!))
    // The icon button beside the microphone is gone.
    expect(desktop).not.toContain('wb-ag-avatar')
    const bottom = desktop.slice(desktop.lastIndexOf('<div class="wb-ag-composer-row">'))
    expect(bottom).toContain('wb-ag-mic')
    expect(bottom).toContain('Start session')
    expect(bottom).not.toContain('Avatar')
    // Off (the store's first state): no note yet.
    expect(desktop).not.toContain('small animated character')
    // The phone folds the options away, the Avatar box with them, until Options… is tapped.
    const phone = composer([claude], true)
    expect(phone).toContain('class="wb-ag-composer compact"')
    expect(phone).toContain('Options…')
    expect(phone).not.toContain('Name (optional)')
    expect(control(phone)).toBeNull()
    expect(phone).not.toContain('wb-ag-avatar')
  })

  it('is offered for every CLI that can have an avatar, not only where Ultracode is', () => {
    // Kimi Code has no Ultracode, Restricted or Remote Control: the box is there all the same.
    const kimiForm = composer([kimi], false)
    expect(control(kimiForm)).not.toBeNull()
    expect(kimiForm).not.toMatch(option('Ultracode'))
    expect(control(composer([codex], false))).not.toBeNull()
    expect(control(composer([without(claude)], false))).toBeNull()
    // The chosen CLI decides, not the others in the picker.
    expect(control(composer([without(claude), kimi], false))).toBeNull()
    expect(control(composer([kimi, without(claude)], false))).not.toBeNull()
  })

  it('shows its help line under the options while it is ticked, also where the phone folds them', () => {
    const first = useAgentsUi.getInitialState()
    const was = first.lastAvatar
    first.lastAvatar = true
    try {
      const desktop = composer([claude], false)
      expect(control(desktop)).toContain('checked=""')
      const help = desktop.indexOf(`<div class="wb-ag-note">${AVATAR_HELP}</div>`)
      expect(help).toBeGreaterThan(desktop.indexOf(control(desktop)!))
      // A remembered choice shows on the phone although its box is folded away.
      const phone = composer([claude], true)
      expect(control(phone)).toBeNull()
      expect(phone).toContain(`<div class="wb-ag-note">${AVATAR_HELP}</div>`)
      // A CLI that cannot have one: no box, no line, and nothing asked for.
      const off = composer([without(claude)], false)
      expect(control(off)).toBeNull()
      expect(off).not.toContain(AVATAR_HELP)
    } finally {
      first.lastAvatar = was
    }
  })

  it('follows the form’s own dev container choice in the whole composer', () => {
    // The control is on: a composer rendered here reads the first state of the store it was loaded with.
    const first = useAgentsUi.getInitialState()
    const was = first.lastAvatar
    first.lastAvatar = true
    try {
      const inside: Dev = { state: 'running', inContainer: true, agents: { codex: '/usr/local/bin/codex', kimi: '/usr/local/bin/kimi', claude: '/usr/local/bin/claude' } }
      const ticked = (html: string) => /<input type="checkbox"[^>]*checked=""[^>]*\/>(?:<svg[^>]*>.*?<\/svg>)? Run in dev container/.test(html)
      // No dev container: the host's sentence, or none.
      expect(composer([kimi], false)).toContain(HOST('Kimi Code'))
      expect(composer([codex], false)).not.toContain('has no Workbench tools')
      expect(composer([kimi], false)).not.toContain('Run in dev container')
      // The project runs its sessions in the container and the CLI is installed there: the box is ticked.
      const codexInside = composer([codex], false, inside)
      expect(ticked(codexInside)).toBe(true)
      expect(codexInside).toContain(CONTAINER('Codex'))
      const kimiInside = composer([kimi], false, inside)
      expect(kimiInside).toContain(CONTAINER('Kimi Code'))
      expect(kimiInside).not.toContain('workbench avatar')
      expect(composer([claude], false, inside)).not.toContain('has no Workbench tools')
      // The container runs, but the project's sessions start on the host by default: not ticked, the host's sentence.
      const onHost = composer([kimi], false, { ...inside, inContainer: false })
      expect(onHost).toContain('Run in dev container')
      expect(ticked(onHost)).toBe(false)
      expect(onHost).toContain(HOST('Kimi Code'))
      expect(composer([codex], false, { ...inside, inContainer: false })).not.toContain('has no Workbench tools')
      // The CLI is not installed in the container: the session can only start on the host.
      const missing = composer([kimi], false, { ...inside, agents: { kimi: null } })
      expect(ticked(missing)).toBe(false)
      expect(missing).toContain(HOST('Kimi Code'))
      // A stopped container is no choice at all.
      expect(composer([codex], false, { ...inside, state: 'stopped' })).not.toContain('has no Workbench tools')
      // The phone's folded form says the same.
      expect(composer([codex], true, inside)).toContain(CONTAINER('Codex'))
    } finally {
      first.lastAvatar = was
    }
  })

  it('is one remembered choice for every composer, kept in wb.agents.v1', async () => {
    const data = new Map<string, string>()
    // The store persists to `window.localStorage`: a browser's, kept in a map.
    vi.stubGlobal('window', {
      localStorage: {
        getItem: (k: string) => data.get(k) ?? null,
        setItem: (k: string, v: string) => void data.set(k, v),
        removeItem: (k: string) => void data.delete(k),
      },
    })
    const stored = () => (JSON.parse(data.get('wb.agents.v1')!) as { state: Record<string, unknown> }).state
    vi.resetModules()
    const fresh = await import('../store')
    expect(fresh.useAgentsUi.getState().lastAvatar).toBe(false)
    fresh.useAgentsUi.getState().setLastAvatar(true)
    expect(fresh.useAgentsUi.getState().lastAvatar).toBe(true)
    expect(stored().lastAvatar).toBe(true)
    // A new page load reads it back, and turning it off is remembered too.
    vi.resetModules()
    const again = await import('../store')
    expect(again.useAgentsUi.getState().lastAvatar).toBe(true)
    again.useAgentsUi.getState().setLastAvatar(false)
    expect(stored().lastAvatar).toBe(false)
    // What a browser stored before the control existed: off, and the rest as it was.
    data.set('wb.agents.v1', JSON.stringify({ state: { lastProvider: 'codex', allProjects: true }, version: 0 }))
    vi.resetModules()
    const before = await import('../store')
    expect(before.useAgentsUi.getState()).toMatchObject({ lastAvatar: false, lastProvider: 'codex', allProjects: true })
  })
})
