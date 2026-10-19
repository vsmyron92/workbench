// The Skills tool window (right): which skills the project's agents can use, where each comes from
// and what the lot costs every session, and the few skills Workbench offers to teach agents to use
// it. A row opens the skill's panel; nothing here changes anything except by opening that panel.

import { useMemo, type MouseEvent } from 'react'
import { ChevronDown, ChevronRight, GraduationCap, Puzzle, RefreshCw, TriangleAlert } from 'lucide-react'
import { Badge, Button, EmptyState, ErrorBox, IconButton, Input, Loading, showMenu, type MenuEntry } from '@/ui'
import { copy, openInEditor, openSkill, titleOf } from './actions'
import { useSkillsReport } from './api'
import {
  catalogState,
  CLI_LABEL,
  compact,
  effectiveFilters,
  groupSkills,
  invokeText,
  isFiltering,
  listed,
  matches,
  matchesCatalog,
  needsAttention,
  oneLine,
  statusWord,
  summarize,
  tokensText,
  worstIssue,
  type AccountSummary,
  type Group,
  type Summary,
} from './logic'
import { CliChips, CliIcon } from './parts'
import { useSkillsUi } from './store'
import type { CatalogEntry, Cli, SkillInfo } from './types'

/** Groups with more skills than this start folded, so a plugin of dozens does not push the rest out of sight. */
const OPEN_UP_TO = 8

export function SkillsToolWindow({ projectId }: { projectId: string | null }) {
  const q = useSkillsReport(projectId)
  const filters = useSkillsUi((s) => s.filters)
  const setFilters = useSkillsUi((s) => s.setFilters)
  const report = q.data

  const view = useMemo(() => {
    if (!report) return null
    const own = listed(report.skills)
    const summary = summarize(report)
    const applied = effectiveFilters(filters, summary.attention)
    const shown = own.filter((s) => matches(s, applied))
    return {
      summary,
      applied,
      groups: groupSkills(shown, report.agents),
      catalog: report.catalog.filter((c) => matchesCatalog(c, applied)),
      shown: shown.length,
      total: own.length,
      multi: report.agents.filter((a) => a.installed || a.skills > 0).length > 1,
    }
  }, [report, filters])

  if (!projectId) return <EmptyState icon={GraduationCap} title="No project selected" />
  if (q.error && !report) return <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
  if (!report || !view) return <Loading />

  const filtering = isFiltering(view.applied)
  return (
    <div className="wb-fill sk">
      <div className="sk-top">
        <Input small placeholder="Filter skills" value={filters.query} onChange={(e) => setFilters({ query: e.target.value })} aria-label="Filter skills" />
        <IconButton icon={RefreshCw} size="small" label="Reload: skills are read from disk each time" onClick={() => void q.refetch()} disabled={q.isFetching} />
      </div>
      <div className="wb-scroll">
        <Summary summary={view.summary} clis={filters.clis} attention={view.applied.attention} shown={filtering ? view.shown : null} onFilter={setFilters} />
        {view.total === 0 && !filtering && (
          <div className="sk-hint-box">
            <strong>No skills found for this project.</strong> A skill is a folder with a <code>SKILL.md</code>: your own live in <code>~/.claude/skills</code>, a project&apos;s in <code>.claude/skills</code> (Codex,
            Gemini CLI and Kimi Code read <code>.agents/skills</code> too).
          </div>
        )}
        {filtering && view.shown === 0 && view.catalog.length === 0 && (
          <div className="sk-hint-box">
            Nothing matches the filter.{' '}
            <Button size="small" onClick={() => setFilters({ query: '', clis: [], attention: false })}>
              Clear filters
            </Button>
          </div>
        )}
        {view.groups.map((g) => (
          <GroupView key={g.key} g={g} projectId={projectId} multi={view.multi} filtering={filtering} />
        ))}
        {view.catalog.length > 0 && (
          <div className="sk-group">
            <div className="wb-section-header sk-ghead static">
              <Puzzle size={13} aria-hidden />
              <span>Workbench</span>
              <span className="wb-subtle">optional</span>
            </div>
            <div className="sk-hint-box tight">Skills that teach an agent to use Workbench&apos;s tools. Nothing is installed until you do.</div>
            {view.catalog.map((c) => (
              <CatalogRow key={c.name} e={c} projectId={projectId} />
            ))}
          </div>
        )}
        {report.notes.map((n) => (
          <div key={n} className="sk-hint-box warn">
            {n}
          </div>
        ))}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- the summary

function agentTitle(r: AccountSummary): string {
  const a = r.account
  const lines = [`${a.label}: ${r.count} skill${r.count === 1 ? '' : 's'} it can load here`, `Folder: ${a.folder}`]
  if (a.session) {
    const when = a.session.atMs ? new Date(a.session.atMs).toLocaleString() : 'its latest session'
    lines.push(`The newest session here (${when}${a.session.version ? `, Claude Code ${a.session.version}` : ''}) received ${a.session.count} skills, ${tokensText(a.session.tokens)}.`)
    lines.push(`Estimated from the files now: ${tokensText(a.listingTokens)}.`)
  } else {
    lines.push(`Listing cost estimated from the files: ${tokensText(a.listingTokens)} in every session.`)
  }
  if (a.cli === 'claude') lines.push('Claude Code keeps the listing within 1% of the model’s context window and shortens descriptions beyond that.')
  return lines.join('\n')
}

function Summary({
  summary,
  clis,
  attention,
  shown,
  onFilter,
}: {
  summary: Summary
  clis: Cli[]
  attention: boolean
  /** How many skills the filters leave (null: no filter). */
  shown: number | null
  onFilter: (f: { clis?: Cli[]; attention?: boolean }) => void
}) {
  const toggleCli = (c: Cli) => onFilter({ clis: clis.includes(c) ? clis.filter((x) => x !== c) : [...clis, c] })
  return (
    <div className="sk-summary">
      <div className="sk-stats">
        <div className="sk-stat">
          <b>{summary.usable}</b>
          <span>{summary.usable === 1 ? 'skill' : 'skills'} available</span>
        </div>
        {summary.inactive > 0 && (
          <div className="sk-stat" title="Present but turned off, or hidden by another skill of the same name">
            <b>{summary.inactive}</b>
            <span>off or hidden</span>
          </div>
        )}
        {summary.attention > 0 && (
          <button type="button" className={`sk-stat button${attention ? ' on' : ''}`} onClick={() => onFilter({ attention: !attention })} title="Skills with something to fix or look at: show only those">
            <b className="warn">{summary.attention}</b>
            <span>to review</span>
          </button>
        )}
        {shown !== null && <span className="wb-subtle wb-small sk-shown">{shown} shown</span>}
      </div>
      {summary.rows.length > 0 && (
        <div className="sk-agents">
          {summary.rows.map((r) => (
            <button
              key={r.account.id}
              type="button"
              className={`sk-agent${clis.includes(r.account.cli) ? ' on' : ''}`}
              onClick={() => toggleCli(r.account.cli)}
              title={agentTitle(r)}
              aria-pressed={clis.includes(r.account.cli)}
            >
              <CliIcon cli={r.account.cli} size={13} />
              <span className="sk-agent-name wb-ellipsis">{r.account.label || CLI_LABEL[r.account.cli]}</span>
              <span className="sk-agent-count">{r.count}</span>
              <span className="sk-agent-tokens" title={r.measured ? 'What its newest session here received' : 'Estimated from the files'}>
                {r.measured ? '' : '≈ '}
                {compact(r.tokens)} tokens
              </span>
            </button>
          ))}
        </div>
      )}
      {summary.powerful > 0 && (
        <div className="sk-note" title="Skills that pre-approve tools, register hooks or run shell commands when invoked. Open one to see which.">
          <TriangleAlert size={12} aria-hidden /> {summary.powerful} can act without asking
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------- the list

function GroupView({ g, projectId, multi, filtering }: { g: Group; projectId: string; multi: boolean; filtering: boolean }) {
  const folded = useSkillsUi((s) => s.folded[g.key])
  const toggle = useSkillsUi((s) => s.toggleFolded)
  const open = filtering ? true : folded === undefined ? g.skills.length <= OPEN_UP_TO : !folded
  return (
    <div className="sk-group">
      <div className="wb-section-header sk-ghead" onClick={() => toggle(g.key, open)}>
        <button
          type="button"
          className="wb-section-toggle"
          aria-expanded={open}
          onClick={(e) => {
            e.stopPropagation()
            toggle(g.key, open)
          }}
        >
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          <span>{g.title}</span>
          <span className="wb-subtle">{g.skills.length}</span>
        </button>
        {g.hint && <span className="wb-subtle sk-ghint wb-ellipsis">{g.hint}</span>}
      </div>
      {open && g.skills.map((s) => <SkillRow key={s.id} s={s} projectId={projectId} multi={multi} group={g} />)}
    </div>
  )
}

function SkillRow({ s, projectId, multi, group }: { s: SkillInfo; projectId: string; multi: boolean; group: Group }) {
  const selected = useSkillsUi((u) => u.selected === s.id)
  const word = statusWord(s)
  const issue = needsAttention(s) ? worstIssue(s) : null
  // Plugin and claude.ai groups already say the namespace.
  const label = s.scope === 'plugin' || s.scope === 'synced' ? s.name : s.qualified
  const ns = label.includes(':') ? label.slice(0, label.indexOf(':') + 1) : ''
  const open = () => openSkill(projectId, s.id, titleOf(s))
  const menu = (e: MouseEvent) => {
    const items: MenuEntry[] = [{ label: 'Open', run: open }]
    if (s.projectFile) items.push({ label: 'Open in editor', run: () => openInEditor(projectId, s.projectFile as string) })
    items.push('separator', { label: 'Copy name', run: () => void copy(s.qualified, 'the name') })
    const cmd = invokeText(s)
    if (cmd) items.push({ label: `Copy ${cmd}`, run: () => void copy(cmd, cmd) })
    if (s.path) items.push({ label: 'Copy path', run: () => void copy(s.path, 'the path') })
    showMenu(e, items)
  }
  return (
    <div
      className={`wb-list-row sk-row${selected ? ' selected' : ''}${s.status === 'disabled' || s.status === 'shadowed' ? ' dim' : ''}`}
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          open()
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault()
        menu(e)
      }}
      title={s.description}
    >
      <div className="sk-main">
        <div className="sk-name wb-ellipsis">
          {ns && <span className="sk-ns">{ns}</span>}
          {ns ? label.slice(ns.length) : label}
          {s.kind === 'command' && <span className="sk-kind">command</span>}
        </div>
        <div className="sk-desc wb-ellipsis">{oneLine(s.description) || <span className="wb-subtle">no description</span>}</div>
      </div>
      <div className="sk-meta">
        {issue && (
          <span className={`sk-attn ${issue.level}`} title={issue.message}>
            <TriangleAlert size={13} aria-label={issue.message} />
          </span>
        )}
        {word && (
          <Badge tone={word.tone} title={s.statusReason}>
            {word.text}
          </Badge>
        )}
        {multi && group.key !== 'builtin' && <CliChips clis={s.agents} />}
      </div>
    </div>
  )
}

function CatalogRow({ e, projectId }: { e: CatalogEntry; projectId: string }) {
  const id = `wb:${e.name}`
  const selected = useSkillsUi((u) => u.selected === id)
  const state = catalogState(e)
  const open = () => openSkill(projectId, id, e.name)
  return (
    <div
      className={`wb-list-row sk-row${selected ? ' selected' : ''}`}
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={(ev) => {
        if (ev.key === 'Enter' || ev.key === ' ') {
          ev.preventDefault()
          open()
        }
      }}
      title={e.description}
    >
      <div className="sk-main">
        <div className="sk-name wb-ellipsis">{e.name}</div>
        <div className="sk-desc wb-ellipsis">{oneLine(e.description)}</div>
      </div>
      <div className="sk-meta">{state && <Badge tone={state.tone}>{state.text}</Badge>}</div>
    </div>
  )
}
