// CLion's interactive rebase dialog: the commits oldest first, an action per row
// (pick / reword / edit / squash / fixup / drop), drag or Alt+↑/↓ to reorder,
// messages edited inline. Runs as a git op (progress card); stops go through the
// Commit window's Continue / Skip / Abort and the conflict panel.

import { useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, ArrowDown, ArrowUp, Cloud, CornerLeftUp, GitCommitHorizontal, GripVertical, RotateCcw } from 'lucide-react'
import { confirmDialog } from '@/shell/actions'
import { Badge, Button, Checkbox, EmptyState, ErrorBox, IconButton, Loading, Modal, Select, TextArea, TimeAgo } from '@/ui'
import { useRebasePlan } from './api'
import { openCommit, runInteractiveRebase } from './actions'
import { shortSha } from './logic'
import {
  ACTIONS,
  actionByKey,
  chainOf,
  editMessage,
  initialRows,
  moveRow,
  problems,
  resetSquashMessage,
  rewrittenPushed,
  setAction,
  squashMessage,
  summary,
  toEntries,
  type RebaseRow,
} from './rebasePlan'
import type { GitDialog } from './store'
import type { RebaseAction } from './types'

type Props = { d: Extract<GitDialog, { kind: 'rebase' }>; onClose: () => void }

export function RebaseDialog({ d, onClose }: Props) {
  const plan = useRebasePlan(d.projectId, d.from, d.onto)
  const p = plan.data
  const [rows, setRows] = useState<RebaseRow[] | null>(null)
  const [sel, setSel] = useState(0)
  const [autostash, setAutostash] = useState(true)
  const [busy, setBusy] = useState(false)
  const [drag, setDrag] = useState<{ from: number; over: number | null } | null>(null)
  const listRef = useRef<HTMLDivElement>(null)

  // First plan: rows as planned, with the preset (e.g. "Edit Commit Message…") applied.
  useEffect(() => {
    if (!p || rows) return
    let r = initialRows(p.commits)
    const at = d.focus ? r.findIndex((x) => x.sha === d.focus) : -1
    if (at >= 0 && d.preset) r = setAction(r, at, d.preset)
    setRows(r)
    setSel(Math.max(0, at))
  }, [p, rows, d.focus, d.preset])

  useEffect(() => {
    listRef.current?.querySelector(`[data-row="${sel}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [sel])

  const issues = useMemo(() => (rows ? problems(rows) : []), [rows])
  const pushedCount = useMemo(() => (rows && p ? rewrittenPushed(rows, p.commits, p.rewritesAll, p.skipped) : 0), [rows, p])
  const skipped = p?.skipped ?? []
  const unchanged = !!rows && !!p && !p.rewritesAll && rows.every((r, i) => r.sha === p.commits[i].sha && r.action === 'pick')

  const update = (f: (r: RebaseRow[]) => RebaseRow[]) => setRows((cur) => (cur ? f(cur) : cur))
  const act = (i: number, a: RebaseAction) => update((r) => setAction(r, i, a))
  const move = (i: number, to: number) => {
    if (!rows || to < 0 || to >= rows.length) return
    update((r) => moveRow(r, i, to))
    setSel(to)
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!rows || (e.target as HTMLElement).closest('textarea, select, input')) return
    if (e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
      e.preventDefault()
      move(sel, sel + (e.key === 'ArrowUp' ? -1 : 1))
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault()
      setSel((s) => Math.max(0, Math.min(rows.length - 1, s + (e.key === 'ArrowUp' ? -1 : 1))))
    } else if (!e.ctrlKey && !e.metaKey && !e.altKey) {
      const a = actionByKey(e.key)
      if (a) {
        e.preventDefault()
        act(sel, a)
      }
    }
  }

  const start = async () => {
    if (!p || !rows || issues.length || busy) return
    if (pushedCount > 0) {
      const target = p.branch ?? 'HEAD'
      const ok = await confirmDialog({
        title: `Rewrite ${pushedCount} pushed commit${pushedCount === 1 ? '' : 's'}?`,
        message: `${pushedCount === 1 ? 'A commit that is' : `${pushedCount} commits that are`} already on ${p.pushedRef ?? 'a remote'} will get new ids. Publishing the result needs a force push, and anyone who built on them has to rebase. Type the branch name to go on.`,
        confirmLabel: 'Rewrite History',
        danger: true,
        typed: target,
      })
      if (!ok) return
    }
    setBusy(true)
    const title = p.onto ? `Rebase ${p.branch ?? 'HEAD'} onto ${p.onto} (interactive)` : `Rebase ${p.branch ?? 'HEAD'} (interactive)`
    const started = await runInteractiveRebase(
      d.projectId,
      { from: d.from, onto: d.onto, head: p.head, entries: toEntries(rows), autostash: p.dirty > 0 && autostash, confirmPushed: pushedCount > 0 },
      title,
    )
    setBusy(false)
    if (started) onClose()
  }

  const title = d.onto ? `Interactive rebase onto ${d.onto}` : `Interactive rebase from ${shortSha(d.from)}`
  const blocked = !!p && (p.merges || p.state !== 'clean' || (p.dirty > 0 && !autostash))
  return (
    <Modal
      title={title}
      wide
      onClose={onClose}
      footer={
        <>
          {rows && p && <span className="wb-small wb-muted git-rb-summary">{summary(rows, p.commits)}</span>}
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant={pushedCount > 0 ? 'danger' : 'primary'}
            loading={busy}
            disabled={!rows || !!issues.length || blocked || unchanged}
            title={unchanged ? 'Change an action or the order first' : undefined}
            onClick={() => void start()}
          >
            Start Rebasing
          </Button>
        </>
      }
    >
      {plan.isLoading && <Loading label="Planning…" />}
      {plan.error && <ErrorBox error={plan.error} onRetry={() => void plan.refetch()} />}
      {p && rows && (
        <div className="git-rb" onKeyDown={onKeyDown}>
          <div className="git-rb-where wb-small wb-muted">
            {p.commits.length} commit{p.commits.length === 1 ? '' : 's'} of <b className="git-mono">{p.branch ?? 'HEAD'}</b>
            {p.onto ? (
              <>
                {' '}replayed onto <b className="git-mono">{p.onto}</b>
              </>
            ) : p.root ? (
              ' from the first commit'
            ) : (
              <>
                {' '}on top of <span className="git-mono">{shortSha(p.base)}</span>
              </>
            )}
            . Oldest first; drag rows or use Alt+↑/↓ to reorder, and P R E S F D to set an action.
          </div>
          {p.state !== 'clean' && (
            <div className="git-banner">
              <AlertTriangle size={14} className="wb-warning" />
              <span className="text">A {p.state.replace('-', ' ')} is in progress: finish or abort it first.</span>
            </div>
          )}
          {p.merges && (
            <div className="git-banner">
              <AlertTriangle size={14} className="wb-warning" />
              <span className="text">The range contains merge commits; an interactive rebase would flatten them. Rebase from a commit after the last merge.</span>
            </div>
          )}
          {p.commits.length === 0 ? (
            <EmptyState icon={GitCommitHorizontal} title="No commits to rebase">
              {p.onto
                ? skipped.length
                  ? `Every commit of ${p.branch ?? 'HEAD'} that is not on ${p.onto} is already there as a cherry-pick: the rebase only moves the branch.`
                  : `${p.branch ?? 'HEAD'} has no commits that are not on ${p.onto}.`
                : 'Nothing is above the chosen commit.'}
            </EmptyState>
          ) : (
            <>
              <div className="git-rb-tools">
                {ACTIONS.map((a) => (
                  <Button
                    key={a.id}
                    size="small"
                    variant={rows[sel]?.action === a.id ? 'primary' : 'default'}
                    title={`${a.hint} (${a.key.toUpperCase()})`}
                    onClick={() => act(sel, a.id)}
                  >
                    {a.label}
                  </Button>
                ))}
                <span className="git-tb-sep" />
                <IconButton size="small" icon={ArrowUp} label="Move up (Alt+↑)" disabled={sel === 0} onClick={() => move(sel, sel - 1)} />
                <IconButton size="small" icon={ArrowDown} label="Move down (Alt+↓)" disabled={sel >= rows.length - 1} onClick={() => move(sel, sel + 1)} />
                <span style={{ flex: 1 }} />
                <Button size="small" variant="ghost" icon={RotateCcw} onClick={() => setRows(initialRows(p.commits))}>
                  Reset
                </Button>
              </div>
              <div className="git-rb-list" ref={listRef} tabIndex={0} role="listbox" aria-label="Commits to rebase">
                {rows.map((r, i) => {
                  const chain = chainOf(rows, i)
                  const head = chain ? chain.head : -1
                  const sm = r.action === 'squash' ? squashMessage(rows, i) : null
                  const dropTarget = drag && drag.over === i && drag.from !== i
                  return (
                    <div key={r.sha} className="git-rb-item" data-row={i}>
                      <div
                        className={`git-rb-row${i === sel ? ' selected' : ''} a-${r.action}${dropTarget ? (drag.from < i ? ' drop-below' : ' drop-above') : ''}`}
                        role="option"
                        aria-selected={i === sel}
                        draggable
                        onClick={() => setSel(i)}
                        onDragStart={(e) => {
                          e.dataTransfer.effectAllowed = 'move'
                          e.dataTransfer.setData('text/plain', r.sha)
                          setDrag({ from: i, over: null })
                          setSel(i)
                        }}
                        onDragOver={(e) => {
                          if (!drag) return
                          e.preventDefault()
                          if (drag.over !== i) setDrag({ ...drag, over: i })
                        }}
                        onDrop={(e) => {
                          e.preventDefault()
                          if (drag) move(drag.from, i)
                          setDrag(null)
                        }}
                        onDragEnd={() => setDrag(null)}
                      >
                        <GripVertical size={14} className="grip" />
                        <Select
                          className="git-rb-action"
                          value={r.action}
                          aria-label={`Action for ${r.commit.subject}`}
                          onClick={(e) => e.stopPropagation()}
                          onFocus={() => setSel(i)}
                          onChange={(e) => act(i, e.target.value as RebaseAction)}
                        >
                          {ACTIONS.map((a) => (
                            <option key={a.id} value={a.id}>
                              {a.label.toLowerCase()}
                            </option>
                          ))}
                        </Select>
                        <a className="git-mono git-link sha" title="Show commit" onClick={() => openCommit(d.projectId, r.sha)}>
                          {shortSha(r.sha)}
                        </a>
                        <span className="subject" title={r.commit.message}>
                          {head >= 0 && <CornerLeftUp size={12} className="into" />}
                          {r.action === 'reword' && r.message.trim() ? r.message.split('\n')[0] : r.commit.subject}
                        </span>
                        {r.commit.pushed && (
                          <span className="pushed" title={`Already on ${p.pushedRef ?? 'a remote'}`}>
                            <Cloud size={12} />
                          </span>
                        )}
                        <span className="author">{r.commit.author}</span>
                        <span className="when">
                          <TimeAgo time={r.commit.time} />
                        </span>
                      </div>
                      {chain && i === sel && (
                        <div className="git-rb-note wb-xs wb-muted">
                          Melds into {shortSha(rows[head].sha)} “{rows[head].commit.subject}”{r.action === 'fixup' ? ', keeping its message' : ''}
                          {r.action === 'squash' && chain.end !== i ? '; the combined message is edited at the last squash of the chain' : ''}
                        </div>
                      )}
                      {r.action === 'reword' && (
                        <TextArea
                          className="git-rb-msg"
                          value={r.message}
                          rows={Math.min(8, Math.max(2, r.message.split('\n').length))}
                          aria-label={`New message of ${r.commit.subject}`}
                          placeholder="New commit message"
                          onFocus={() => setSel(i)}
                          onChange={(e) => update((rs) => editMessage(rs, i, e.target.value))}
                        />
                      )}
                      {sm && (
                        <>
                          <TextArea
                            className="git-rb-msg"
                            value={sm.text}
                            rows={Math.min(8, Math.max(2, sm.text.split('\n').length))}
                            aria-label="Message of the combined commit"
                            placeholder="Message of the combined commit (empty: git's combined message)"
                            onFocus={() => setSel(i)}
                            onChange={(e) => update((rs) => editMessage(rs, i, e.target.value))}
                          />
                          {sm.edited && (
                            <div className={`git-rb-msgnote wb-xs${sm.stale ? ' stale' : ' wb-muted'}`}>
                              {sm.stale && <AlertTriangle size={12} className="wb-warning" />}
                              <span>
                                {sm.stale
                                  ? 'The commits of this squash changed since you edited the message.'
                                  : 'Edited: it no longer follows the commits of the chain.'}
                              </span>
                              <Button size="small" variant="ghost" onClick={() => update((rs) => resetSquashMessage(rs, i))}>
                                Use the Combined Message
                              </Button>
                            </div>
                          )}
                        </>
                      )}
                    </div>
                  )
                })}
              </div>
              {skipped.length > 0 && (
                <div className="git-rb-skipped">
                  <div className="wb-xs wb-muted">
                    Already on <b className="git-mono">{p.onto}</b> (cherry-picked): git leaves {skipped.length === 1 ? 'this commit' : 'these commits'} out, so{' '}
                    {skipped.length === 1 ? 'it disappears' : 'they disappear'} from {p.branch ?? 'HEAD'}.
                  </div>
                  {skipped.map((c) => (
                    <div key={c.sha} className="git-rb-row skipped">
                      <a className="git-mono git-link sha" title="Show commit" onClick={() => openCommit(d.projectId, c.sha)}>
                        {shortSha(c.sha)}
                      </a>
                      <span className="subject" title={c.message}>
                        {c.subject}
                      </span>
                      {c.pushed && (
                        <span className="pushed" title={`Already on ${p.pushedRef ?? 'a remote'}`}>
                          <Cloud size={12} />
                        </span>
                      )}
                      <span className="author">{c.author}</span>
                      <span className="when">
                        <TimeAgo time={c.time} />
                      </span>
                    </div>
                  ))}
                </div>
              )}
              {issues.map((m) => (
                <div key={m} className="wb-small wb-danger">
                  {m}
                </div>
              ))}
              {pushedCount > 0 && (
                <div className="git-banner">
                  <Cloud size={14} className="wb-warning" />
                  <span className="text">
                    {pushedCount} commit{pushedCount === 1 ? ' is' : 's are'} already on {p.pushedRef ?? 'a remote'} and will be rewritten: publishing needs a force push.
                  </span>
                </div>
              )}
              {p.dirty > 0 && (
                <div className={`git-banner${autostash ? ' info' : ''}`}>
                  <span className="text">
                    {p.dirty} file{p.dirty === 1 ? ' has' : 's have'} uncommitted changes.{' '}
                    {autostash ? 'They are stashed first and put back afterwards.' : 'Commit, stash or shelve them first, or use autostash.'}
                  </span>
                  <Checkbox checked={autostash} onChange={setAutostash}>
                    Autostash
                  </Checkbox>
                </div>
              )}
              {p.rewritesAll && (
                <div className="wb-xs wb-muted">
                  <Badge>onto {p.onto}</Badge> every commit is replayed on the new base and gets a new id.
                </div>
              )}
            </>
          )}
        </div>
      )}
    </Modal>
  )
}
