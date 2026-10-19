// Small presentational pieces of the skills views.

import type { ReactNode } from 'react'
import { Asterisk, CircleAlert, CodeXml, Info, Moon, Sparkles, TriangleAlert, type LucideIcon } from 'lucide-react'
import { Badge } from '@/ui'
import { CLI_LABEL, INSTALL_WORD } from './logic'
import type { Cli, InstallState, Issue, SkillInfo } from './types'

const CLI_ICON: Record<Cli, LucideIcon> = { claude: Asterisk, codex: CodeXml, kimi: Moon, gemini: Sparkles }

/** The icon of an agent CLI, tinted like the agents column's. */
export function CliIcon({ cli, size = 12 }: { cli: Cli; size?: number }) {
  const Icon = CLI_ICON[cli]
  return <Icon size={size} className={`sk-cli ${cli}`} aria-hidden />
}

/** Which CLIs read a skill: their icons, named in the tooltip. */
export function CliChips({ clis }: { clis: Cli[] }) {
  const names = clis.map((c) => CLI_LABEL[c]).join(', ')
  return (
    <span className="sk-clis" title={`Read by ${names}`} aria-label={`Read by ${names}`}>
      {clis.map((c) => (
        <CliIcon key={c} cli={c} />
      ))}
    </span>
  )
}

const LEVEL_ICON: Record<Issue['level'], LucideIcon> = { error: CircleAlert, warn: TriangleAlert, info: Info }

export function IssueList({ issues }: { issues: Issue[] }) {
  if (!issues.length) return null
  return (
    <ul className="sk-issues">
      {issues.map((i, n) => {
        const Icon = LEVEL_ICON[i.level]
        return (
          <li key={`${i.code}:${n}`} className={`sk-issue ${i.level}`}>
            <Icon size={14} aria-hidden />
            <span>{i.message}</span>
          </li>
        )
      })}
    </ul>
  )
}

interface Chip {
  text: string
  title: string
  tone?: 'warning'
}

/** What a skill can do on its own or how it is started: the things worth knowing before trusting it. */
export function chipsOf(s: SkillInfo): Chip[] {
  const f = s.flags
  const chips: Chip[] = []
  if (f.preapproves.length) {
    chips.push({
      text: `pre-approves ${f.preapproves.length} tool${f.preapproves.length === 1 ? '' : 's'}`,
      title: `While it runs, these tools skip your approval: ${f.preapproves.join(', ')}`,
      tone: 'warning',
    })
  }
  if (f.hooks) chips.push({ text: 'registers hooks', title: 'Hooks run commands around the agent’s tool calls while the skill is active', tone: 'warning' })
  if (f.shell) {
    chips.push({ text: 'runs shell commands', title: 'Claude Code runs the !`command` parts of its text when it is invoked, before the model reads it', tone: 'warning' })
  }
  if (f.scripts) chips.push({ text: 'has scripts', title: 'Its folder has a scripts folder that the agent may run' })
  if (f.forks) chips.push({ text: `sub-agent: ${f.forks}`, title: 'It runs in a sub-agent of its own, not in the main conversation' })
  if (f.manualOnly) chips.push({ text: 'manual only', title: 'Only you start it; the model does not use it by itself' })
  if (f.modelOnly) chips.push({ text: 'model only', title: 'Only the model uses it; it is not in your / menu' })
  if (f.paths.length) chips.push({ text: `only for ${f.paths.join(', ')}`, title: 'It loads only while files matching these patterns are in play' })
  if (f.model) chips.push({ text: `model: ${f.model}`, title: 'It switches the model for the turn it runs in' })
  return chips
}

export function Chips({ s }: { s: SkillInfo }) {
  const chips = chipsOf(s)
  if (!chips.length) return null
  return (
    <div className="sk-chips">
      {chips.map((c) => (
        <Badge key={c.text} tone={c.tone} title={c.title}>
          {c.text}
        </Badge>
      ))}
    </div>
  )
}

export function InstallBadge({ state }: { state: InstallState }) {
  const w = INSTALL_WORD[state]
  return <Badge tone={w.tone}>{w.text}</Badge>
}

/** A label and its value, in the facts grid of a skill's panel. */
export function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="sk-fact">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  )
}
