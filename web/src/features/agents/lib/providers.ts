// Pure helpers over agent providers (Claude Code, Codex, Kimi, Gemini, Aider, custom
// CLIs). No React.

import type { AgentProvider, TerminalInfo } from '@/api/types'
import type { PermissionPreset, ProviderInfo } from '../api'

/** The provider id of an agent terminal (`claude` for records from before providers). */
export function providerIdOf(t: Pick<TerminalInfo, 'agent'>): string {
  return t.agent?.providerId ?? 'claude'
}

export function providerKindOf(t: Pick<TerminalInfo, 'agent'>): AgentProvider {
  return t.agent?.provider ?? 'claude'
}

const KIND_LABEL: Record<AgentProvider, string> = { claude: 'Claude', codex: 'Codex', kimi: 'Kimi', gemini: 'Gemini', aider: 'Aider', custom: 'Agent' }

/**
 * Whether starting an agent terminal again continues its conversation: by id, or (Aider)
 * by restoring the repository's chat history. Custom CLIs start over.
 */
export function resumes(t: Pick<TerminalInfo, 'agent'>): boolean {
  return providerKindOf(t) !== 'custom'
}

/**
 * A dialog state known only from the session's screen (a heuristic): every CLI but Claude
 * Code, whose hooks report its dialogs.
 */
export function dialogSeenOnScreen(t: Pick<TerminalInfo, 'agent'>): boolean {
  const s = t.agent?.state
  return providerKindOf(t) !== 'claude' && (s === 'needs_permission' || s === 'needs_input')
}

export const SEEN_ON_SCREEN = "Recognized on the session's screen (this CLI does not report its dialogs): answer it in the terminal."

/** Whether Workbench learns the session's answers (Claude's hooks, Codex's rollout). */
export function reportsAnswers(kind: AgentProvider): boolean {
  return kind === 'claude' || kind === 'codex'
}

/**
 * What a provider is called in lists: its label, and for a second account of a CLI (its own
 * folder, a name other than the CLI's) the CLI first, so "Work" is not read as a CLI.
 */
export function displayLabel(p: Pick<ProviderInfo, 'id' | 'kind' | 'label' | 'home'>): string {
  if (!p.home || p.id === p.kind) return p.label
  const cli = KIND_LABEL[p.kind]
  return p.label.toLowerCase().includes(cli.toLowerCase()) ? p.label : `${cli} · ${p.label}`
}

/** A short name for the provider of a session: its configured label, else the kind's. */
export function providerLabel(t: Pick<TerminalInfo, 'agent'>, providers?: ProviderInfo[]): string {
  const id = providerIdOf(t)
  const p = providers?.find((x) => x.id === id)
  if (p) return p.kind === 'claude' && p.id === 'claude' ? 'Claude' : displayLabel(p)
  const kind = providerKindOf(t)
  return kind === 'custom' ? id : KIND_LABEL[kind]
}

/**
 * The provider the composer starts with: the one last used (if it can still start),
 * else the configured default, else the first available one.
 */
export function initialProvider(providers: ProviderInfo[], remembered: string | null | undefined, defaultId: string): string | null {
  const usable = (id: string | null | undefined) => providers.find((p) => p.id === id && p.enabled && p.available)
  return usable(remembered)?.id ?? usable(defaultId)?.id ?? providers.find((p) => p.enabled && p.available)?.id ?? providers.find((p) => p.enabled)?.id ?? null
}

/** When an account's limit ends: "3:45 PM" today, "Mon 12:00 AM" within the week, else "Oct 9, 3:00 PM". */
export function limitEnds(ms: number, now: number = Date.now()): string {
  const d = new Date(ms)
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  if (d.toDateString() === new Date(now).toDateString()) return time
  if (ms - now < 6 * 24 * 3600 * 1000) return `${d.toLocaleDateString([], { weekday: 'short' })} ${time}`
  return `${d.toLocaleDateString([], { month: 'short', day: 'numeric' })}, ${time}`
}

/** What the picker says about an account that is at its limit (`null`: nothing). */
export function limitNote(p: Pick<ProviderInfo, 'usage'>, now: number = Date.now()): string | null {
  const u = p.usage
  return u?.limited && u.limitedUntil && u.limitedUntil > now ? `at limit until ${limitEnds(u.limitedUntil, now)}` : null
}

/** Providers shown in the picker: enabled ones, available first (stable otherwise). */
export function pickerProviders(providers: ProviderInfo[]): ProviderInfo[] {
  const on = providers.filter((p) => p.enabled)
  return [...on.filter((p) => p.available), ...on.filter((p) => !p.available)]
}

/** Providers with a conversation history (tabs in the history list). */
export function historyProviders(providers: ProviderInfo[]): ProviderInfo[] {
  return providers.filter((p) => p.enabled && p.supports.history)
}

export function presetOf(p: ProviderInfo | undefined, id: string | null | undefined): PermissionPreset | undefined {
  return id ? p?.permissionModes.find((m) => m.id === id) : undefined
}

/** A permission mode that skips approvals (and Codex's sandbox). */
export function isDangerous(p: ProviderInfo | undefined, id: string | null | undefined): boolean {
  return !!presetOf(p, id)?.dangerous
}

/** What the composer says about how a provider's state is known. */
export function stateNote(p: ProviderInfo): string | null {
  switch (p.stateSource) {
    case 'rollout':
      return `${p.label} reports turns through its session log: Working, Done and the last answer are exact; approval prompts are recognized on its screen.`
    case 'activity':
      switch (p.kind) {
        case 'kimi':
        case 'gemini':
          return `${p.label} reports nothing to Workbench: Working and Idle are estimated from terminal output; approval and question dialogs are recognized on its screen (answer them in the terminal).`
        case 'aider':
          return `${p.label} reports nothing to Workbench: Working and Idle are estimated from terminal output; its (Y)es/(N)o confirmations are recognized on its screen. Starting it again restores the repository's chat.`
        default:
          return `${p.label} reports nothing to Workbench: Working and Idle are estimated from terminal output. "Ask agent" always starts a new ${p.label} session.`
      }
    default:
      return null
  }
}

/** The config.toml snippet shown in the composer's help. */
export const PROVIDER_CONFIG_EXAMPLE = `[agents]
answer_permissions = true      # answer Claude's permission prompts from Workbench
permission_wait = 600          # seconds a request stays answerable here

[agents.providers.codex]      # built-in preset; every field optional
model = "gpt-5.5"
effort = "high"                # -c model_reasoning_effort=…
args = ["--search"]            # extra arguments
env = { CODEX_HOME = "~/.codex" }

[agents.providers.kimi]
enabled = false                # hide a preset (also gemini, aider)

[agents.providers.aider]
args = ["--no-auto-commits"]

[agents.providers.opencode]    # any other agent CLI
command = "opencode"
label = "OpenCode"
install_hint = "npm install -g opencode-ai"`

/** How a conversation goes to another account: itself, as text, or as a short note. */
export type Carry = 'resume' | 'digest' | 'notes'

/**
 * What a move from a `from` CLI to a `to` CLI carries (the server's `conversation::how`): the same CLI
 * resumes the conversation itself; Claude Code and Codex are read and sent as text to another CLI;
 * the others' files are not read.
 */
export function carryKind(from: AgentProvider, to: AgentProvider, transfer: 'conversation' | 'notes' = 'conversation'): Carry {
  const readable = from === 'claude' || from === 'codex'
  if (transfer === 'notes' || !readable) return 'notes'
  return from === to ? 'resume' : 'digest'
}

/** What the move dialog says about a target. */
export function carryText(c: Carry, to: Pick<ProviderInfo, 'local' | 'kind'>, from: AgentProvider): string {
  const where = to.local ? 'It stays on your network.' : from === to.kind ? '' : 'It is sent to that CLI’s service.'
  switch (c) {
    case 'resume':
      return `The conversation itself continues there.${where ? ` ${where}` : ''}`
    case 'digest':
      return `What was said, and a line for each tool call, is written out as text for the new session to read. ${where}`.trim()
    default:
      return 'Only a short note on where it stopped.'
  }
}

/** The toast for a finished move, by how the server carried it (`meta.transfer.mode`). */
export function movedToast(mode: unknown, to: string): { title: string; detail: string } {
  switch (mode) {
    case 'resume':
      return { title: `Continued on ${to}`, detail: 'The same conversation, resumed.' }
    case 'digest':
      return { title: `Continued on ${to}`, detail: 'The conversation was written out as text for it to read.' }
    default:
      return { title: `Started on ${to}`, detail: 'It got a short note on where the old session stopped.' }
  }
}
