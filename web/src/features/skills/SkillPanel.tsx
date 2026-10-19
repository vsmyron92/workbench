// The 'skill' panel: one skill, read-only: what it says it is for, where it is, who reads it, what
// it costs and what it can do without asking, what is wrong with it, its files, and its text. For
// one of Workbench's own skills (id `wb:<name>`) it is also where it is installed and removed:
// the files and the folder are shown first, because an install writes into an agent's own folder.

import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Copy, FileText, Folder, GraduationCap, Puzzle, RefreshCw, SquareTerminal, SquarePen } from 'lucide-react'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import type { PanelProps } from '@/shell/types'
import { Badge, Button, EmptyState, ErrorBox, IconButton, Loading, Markdown } from '@/ui'
import { copy, openInEditor, titleOf } from './actions'
import { sk, skillsApi, useSkillDetail, useSkillFile, useSkillsReport } from './api'
import { bytesText, CLI_LABEL, installChoices, invokeText, scopeTitle, skillLink, tokensText, type InstallChoice } from './logic'
import { Chips, CliChips, CliIcon, Fact, InstallBadge, IssueList } from './parts'
import type { AgentReport, CatalogEntry, SkillDetail, SkillFileEntry, SkillInfo } from './types'

export interface SkillParams {
  projectId: string
  skillId: string
}

export function SkillPanel({ params, setTitle }: PanelProps<SkillParams>) {
  const { projectId, skillId } = params
  const q = useSkillDetail(projectId, skillId)
  const report = useSkillsReport(projectId)
  const d = q.data
  const name = d?.skill?.qualified ?? d?.catalog?.name ?? ''
  useEffect(() => {
    if (name) setTitle(titleOf({ qualified: name }))
  }, [name, setTitle])

  if (q.error && !d) {
    const gone = (q.error as { status?: number }).status === 404
    return gone ? (
      <EmptyState icon={GraduationCap} title="This skill is not there any more" action={<Button size="small" icon={RefreshCw} onClick={() => void q.refetch()}>Look again</Button>}>
        It was moved or removed since the list was made. Reload the Skills window to see what is there now.
      </EmptyState>
    ) : (
      <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
    )
  }
  if (!d) return <Loading />
  return <Detail projectId={projectId} skillId={skillId} d={d} agents={report.data?.agents ?? []} onReload={() => void q.refetch()} />
}

function Detail({ projectId, skillId, d, agents, onReload }: { projectId: string; skillId: string; d: SkillDetail; agents: AgentReport[]; onReload: () => void }) {
  const info = d.skill
  const entry = d.catalog ?? null
  const [current, setCurrent] = useState('SKILL.md')
  // A file that went away with a reload falls back to the skill's text.
  const known = useMemo(() => new Set(['SKILL.md', ...d.files.filter((f) => !f.dir).map((f) => f.path)]), [d.files])
  const shown = known.has(current) ? current : 'SKILL.md'
  const cmd = info ? invokeText(info) : null
  const description = info?.description ?? entry?.description ?? ''
  // A project skill that links out of the repository: Workbench read nothing of it.
  const unread = !!info?.issues.some((i) => i.code === 'link_out')
  return (
    <div className="wb-fill sk-panel">
      <div className="sk-head">
        <div className="wb-row" style={{ alignItems: 'flex-start', gap: 8 }}>
          <span className="sk-head-icon">{info?.kind === 'command' ? <SquareTerminal size={18} /> : entry ? <Puzzle size={18} /> : <GraduationCap size={18} />}</span>
          <h2 className="wb-grow wb-ellipsis">{info?.qualified ?? entry?.name}</h2>
          {cmd && <IconButton icon={Copy} size="small" label={`Copy ${cmd}`} onClick={() => void copy(cmd, cmd)} />}
          {info?.projectFile && <IconButton icon={SquarePen} size="small" label="Open the file in the editor" onClick={() => openInEditor(projectId, info.projectFile as string)} />}
          <IconButton icon={RefreshCw} size="small" label="Reload" onClick={onReload} />
        </div>
        <div className="sk-sub">
          {info ? <span>{scopeTitle(info)}</span> : <span>Workbench skill · optional</span>}
          {info && info.status !== 'active' && (
            <Badge tone={info.status === 'name-only' ? undefined : 'warning'} title={info.statusReason}>
              {info.status === 'disabled' ? 'off' : info.status === 'shadowed' ? 'hidden' : 'name only'}
            </Badge>
          )}
          {info && info.agents.length > 0 && <CliChips clis={info.agents} />}
          {info?.path && <span className="wb-subtle sk-path wb-ellipsis">{info.path}</span>}
        </div>
      </div>
      <div className="wb-scroll">
        <div className="sk-body">
          {entry && <InstallCard entry={entry} agents={agents} />}
          {info?.kind === 'builtin' && (
            <div className="sk-card tone-info">
              Claude Code listed this skill in its newest session in this project, and no skill folder here holds it: it is built into Claude Code, or it was removed after that session began. There are no files to show; this is
              what Claude Code told that session about it.
            </div>
          )}
          <section className="sk-card">
            <p className="sk-about">{description || <span className="wb-subtle">{unread ? 'Not read: see the warning below.' : 'This skill has no description.'}</span>}</p>
            {!unread && info?.descriptionFromText && !info.issues.some((i) => i.code === 'front_matter') && <p className="wb-subtle wb-small">The file has no description, so agents list the first line of its text instead.</p>}
            {!unread && info?.descriptionFromText && info.issues.some((i) => i.code === 'front_matter') && <p className="wb-subtle wb-small">This is the first line of its text, which agents list instead.</p>}
            {info?.whenToUse && (
              <p className="wb-small">
                <strong>When to use:</strong> {info.whenToUse}
              </p>
            )}
            {info && <Chips s={info} />}
          </section>
          {info && <IssueList issues={info.issues} />}
          {info ? <Facts s={info} agents={agents} unread={unread} /> : entry ? <CatalogFacts e={entry} /> : null}
          {d.text !== null && (
            <section className="sk-section">
              {d.files.length > 0 && <Files files={d.files} truncated={d.filesTruncated} current={shown} onPick={setCurrent} />}
              <Viewer projectId={projectId} skillId={skillId} path={shown} skillText={d.text} skillTruncated={d.truncated} known={known} onPick={setCurrent} />
            </section>
          )}
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- facts

function Facts({ s, agents, unread }: { s: SkillInfo; agents: AgentReport[]; unread: boolean }) {
  const accounts = s.accounts.map((id) => agents.find((a) => a.id === id)).filter((a): a is AgentReport => !!a)
  const cost =
    s.tokens > 0
      ? s.scope === 'nested'
        ? `${tokensText(s.tokens)}, once an agent reads a file below ${s.scopeLabel || 'its folder'}: not in every session`
        : `${tokensText(s.tokens)} in every session`
      : s.flags.manualOnly
        ? 'none: only you start it, so it is not in the listing'
        : s.status === 'name-only'
          ? 'its name only'
          : s.status !== 'active'
            ? 'none: it is not loaded'
            : '—'
  return (
    <dl className="sk-facts">
      {s.path && (
        <Fact label="Where">
          <span className="mono sk-where">{s.path}</span>
        </Fact>
      )}
      <Fact label="Read by">
        <span className="wb-row" style={{ flexWrap: 'wrap', gap: 8 }}>
          {s.agents.map((c) => (
            <span key={c} className="wb-row" style={{ gap: 4 }}>
              <CliIcon cli={c} size={13} />
              {CLI_LABEL[c]}
            </span>
          ))}
          {accounts.length > 1 && <span className="wb-subtle">({accounts.map((a) => a.label).join(', ')})</span>}
        </span>
      </Fact>
      {s.invoke.length > 0 && (
        <Fact label="Start it with">
          {s.invoke.map((i) => (
            <code key={i.agent} className="sk-code" title={CLI_LABEL[i.agent]}>
              {i.text}
            </code>
          ))}
          {s.flags.argumentHint && <span className="wb-subtle"> {s.flags.argumentHint}</span>}
        </Fact>
      )}
      {!unread && <Fact label="Costs">{cost}</Fact>}
      {s.kind !== 'builtin' && !unread && (
        <Fact label="Size">
          {bytesText(s.bytes)} · {s.lines} line{s.lines === 1 ? '' : 's'}
          {s.files > 0 ? ` · ${s.files} more file${s.files === 1 ? '' : 's'}` : ''}
        </Fact>
      )}
      {s.inSession !== undefined && !unread && (
        <Fact label="Last session">
          {s.inSession ? 'Listed.' : 'Not in its listing: added since, or left out for lack of room.'}
        </Fact>
      )}
    </dl>
  )
}

function CatalogFacts({ e }: { e: CatalogEntry }) {
  return (
    <dl className="sk-facts">
      <Fact label="Read by">
        <span className="wb-row" style={{ gap: 8 }}>
          <CliIcon cli="claude" size={13} /> Claude Code <CliIcon cli="codex" size={13} /> Codex
        </span>
      </Fact>
      <Fact label="Size">
        {bytesText(e.bytes)} · {e.lines} line{e.lines === 1 ? '' : 's'} · {e.files} file{e.files === 1 ? '' : 's'}
      </Fact>
      <Fact label="Costs">
        {tokensText(Math.ceil((e.name.length + 4 + Math.min(e.description.length, 1536)) / 4))} in every session of an account that has it; the text loads only when an agent uses the skill.
      </Fact>
    </dl>
  )
}

// ---------------------------------------------------------------- install

function InstallCard({ entry, agents }: { entry: CatalogEntry; agents: AgentReport[] }) {
  const qc = useQueryClient()
  const [busy, setBusy] = useState<string | null>(null)
  const run = async (account: AgentReport | undefined, accountId: string, c: InstallChoice, path: string) => {
    const label = account?.label ?? accountId
    if (c.force) {
      const ok = await confirmDialog({
        title: c.run === 'install' ? `Restore ${entry.name} for ${label}?` : `Remove ${entry.name} from ${label}?`,
        message: c.run === 'install' ? `You changed files of this skill in ${path}. Restoring puts Workbench's version back over your edits.` : `You changed files of this skill in ${path}. Removing deletes the files Workbench installed, your edits included.`,
        confirmLabel: c.run === 'install' ? 'Restore' : 'Remove',
        danger: true,
      })
      if (!ok) return
    } else if (c.run === 'remove') {
      const ok = await confirmDialog({ title: `Remove ${entry.name} from ${label}?`, message: `Deletes the files Workbench installed in ${path}. Anything you added there stays.`, confirmLabel: 'Remove', danger: true })
      if (!ok) return
    }
    setBusy(`${accountId}:${c.label}`)
    try {
      if (c.run === 'install') await skillsApi.install(entry.name, accountId, c.force)
      else await skillsApi.remove(entry.name, accountId, c.force)
      toast('success', c.run === 'install' ? `${entry.name} installed for ${label}` : `${entry.name} removed from ${label}`, { timeout: 2500 })
      await qc.invalidateQueries({ queryKey: sk.all })
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(null)
    }
  }
  return (
    <section className="sk-card install">
      <div className="sk-card-title">Install for an agent account</div>
      {entry.installs.length === 0 ? (
        <p className="wb-small wb-muted">
          Workbench&apos;s tools reach Claude Code and Codex sessions, and neither is set up on this computer, so there is nowhere to install this. Add an account in Settings, Agents.
        </p>
      ) : (
        <>
          <p className="wb-small wb-muted">
            Copies the files below into the account&apos;s own skills folder, with a small marker so Workbench knows them again. Nothing else is changed, and a running session may need <code>/reload-skills</code> (Claude Code) or a restart to see it.
          </p>
          <ul className="sk-installs">
            {entry.installs.map((i) => {
              const a = agents.find((x) => x.id === i.account)
              const choices = installChoices(i.state)
              return (
                <li key={i.account} className="sk-install">
                  <div className="sk-install-main">
                    {a && <CliIcon cli={a.cli} size={14} />}
                    <span className="sk-install-who">{a?.label ?? i.account}</span>
                    <InstallBadge state={i.state} />
                    <span className="wb-grow" />
                    {choices.map((c) => (
                      <Button key={c.label} size="small" variant={c.run === 'install' && !c.force ? 'primary' : c.force ? 'danger' : 'default'} loading={busy === `${i.account}:${c.label}`} disabled={busy !== null} onClick={() => void run(a, i.account, c, i.path)}>
                        {c.label}
                      </Button>
                    ))}
                  </div>
                  <div className="mono wb-subtle sk-install-path">{i.path}</div>
                  {i.state === 'foreign' && <div className="wb-small wb-subtle">A folder of this name that Workbench did not make is there; it is left alone.</div>}
                </li>
              )
            })}
          </ul>
        </>
      )}
    </section>
  )
}

// ---------------------------------------------------------------- files and text

interface Row {
  entry: SkillFileEntry
  depth: number
  name: string
}

function rowsOf(files: SkillFileEntry[]): Row[] {
  return files.map((f) => {
    const parts = f.path.split('/')
    return { entry: f, depth: parts.length - 1, name: parts[parts.length - 1] }
  })
}

function Files({ files, truncated, current, onPick }: { files: SkillFileEntry[]; truncated: boolean; current: string; onPick: (p: string) => void }) {
  const [open, setOpen] = useState(true)
  const rows = useMemo(() => rowsOf(files), [files])
  const count = files.filter((f) => !f.dir).length
  return (
    <div className="sk-files-box">
      <button type="button" className="sk-files-title" aria-expanded={open} onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span>Files</span>
        <span className="wb-subtle">{count + 1}</span>
      </button>
      {open && (
        <ul className="sk-files">
          <FileRow depth={0} name="SKILL.md" size={null} active={current === 'SKILL.md'} onPick={() => onPick('SKILL.md')} />
          {rows.map((r) =>
            r.entry.dir ? (
              <li key={r.entry.path} className="sk-file dir" style={{ paddingLeft: 8 + r.depth * 14 }}>
                <Folder size={13} aria-hidden />
                <span>{r.name}</span>
              </li>
            ) : (
              <FileRow key={r.entry.path} depth={r.depth} name={r.name} size={r.entry.size} active={current === r.entry.path} onPick={() => onPick(r.entry.path)} />
            ),
          )}
          {truncated && <li className="sk-file wb-subtle">The list is cut short.</li>}
        </ul>
      )}
    </div>
  )
}

function FileRow({ depth, name, size, active, onPick }: { depth: number; name: string; size: number | null; active: boolean; onPick: () => void }) {
  return (
    <li className={`sk-file${active ? ' active' : ''}`} style={{ paddingLeft: 8 + depth * 14 }}>
      <button type="button" onClick={onPick} aria-current={active ? 'true' : undefined}>
        <FileText size={13} aria-hidden />
        <span className="wb-grow wb-ellipsis">{name}</span>
        {size !== null && <span className="wb-subtle wb-xs">{bytesText(size)}</span>}
      </button>
    </li>
  )
}

const isMarkdownFile = (p: string) => /\.(md|markdown)$/i.test(p)

function Viewer({
  projectId,
  skillId,
  path,
  skillText,
  skillTruncated,
  known,
  onPick,
}: {
  projectId: string
  skillId: string
  path: string
  skillText: string
  skillTruncated: boolean
  known: Set<string>
  onPick: (p: string) => void
}) {
  const own = path === 'SKILL.md'
  const file = useSkillFile(projectId, skillId, own ? null : path)
  const [source, setSource] = useState(false)
  const text = own ? skillText : (file.data?.text ?? null)
  const truncated = own ? skillTruncated : !!file.data?.truncated
  const md = isMarkdownFile(path)
  return (
    <div className="sk-viewer">
      <div className="sk-viewer-bar">
        <span className="mono wb-ellipsis wb-grow">{path}</span>
        {truncated && <Badge tone="warning">cut short</Badge>}
        {md && (
          <div className="wb-tabs sk-mini-tabs" role="tablist">
            <button type="button" role="tab" aria-selected={!source} className={`wb-tab${!source ? ' active' : ''}`} onClick={() => setSource(false)}>
              Rendered
            </button>
            <button type="button" role="tab" aria-selected={source} className={`wb-tab${source ? ' active' : ''}`} onClick={() => setSource(true)}>
              Source
            </button>
          </div>
        )}
      </div>
      <div className="sk-viewer-body">
        {!own && file.error && !file.data ? (
          <ErrorBox error={file.error} onRetry={() => void file.refetch()} />
        ) : !own && !file.data ? (
          <Loading />
        ) : text === null ? (
          <div className="wb-subtle wb-small sk-pad">This is a binary file: it is not shown.</div>
        ) : md && !source ? (
          <div className="sk-pad">
            <Markdown
              text={text}
              onLinkClick={(href) => {
                const target = skillLink(path, href, known)
                if (target === null) return false
                onPick(target)
                return true
              }}
            />
          </div>
        ) : (
          <pre className="sk-source">{text}</pre>
        )}
      </div>
    </div>
  )
}

