// Phone view (mobile tab 'git'): changes with tap-to-stage, commit, pull/push.
// No Monaco here; diffs and conflict resolution are desktop work.

import { useMemo, useState } from 'react'
import { AlertTriangle, ArrowDownToLine, ArrowUpFromLine, Check, CloudDownload, FileText, GitBranch, Sparkles } from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import { Button, Checkbox, EmptyState, ErrorBox, IconButton, Loading, TextArea } from '@/ui'
import { gitApi, isNotRepo, useGitStatus } from './api'
import { BisectBanner } from './Bisect'
import { askCommitMessage, fetchAll, openPush, sequencer, stagePaths, unstagePaths, updateProject } from './actions'
import { groupStatus, sectionCode, shortSha, splitPath, stateLabel, statusClass, type SectionId } from './logic'
import { useDraft, useDrafts } from './store'
import type { GitStatusFile } from './types'

const TITLES: Record<SectionId, string> = { conflicts: 'Conflicts', staged: 'Staged', unstaged: 'Changes', untracked: 'Unversioned' }

export function MobileGit({ projectId }: { projectId: string | null }) {
  if (!projectId) return <EmptyState title="No project selected" />
  return <MobileGitView key={projectId} pid={projectId} />
}

function MobileGitView({ pid }: { pid: string }) {
  const st = useGitStatus(pid)
  const s = st.data
  const sections = useMemo(() => groupStatus(s?.files ?? []), [s])
  const draft = useDraft(pid)
  const update = useDrafts((x) => x.update)
  const [busy, setBusy] = useState(false)

  if (st.error) return isNotRepo(st.error) ? <EmptyState title="Not a git repository" /> : <ErrorBox error={st.error} onRetry={() => void st.refetch()} />
  if (!s) return <Loading />

  const toggle = (section: SectionId, f: GitStatusFile) => {
    if (section === 'staged') void unstagePaths(pid, [f.path])
    else void stagePaths(pid, [f.path])
  }
  const canCommit = !busy && s.state !== 'rebasing' && !sections.conflicts.length && (draft.amend || (sections.staged.length > 0 && !!draft.message.trim()))
  const commit = async () => {
    setBusy(true)
    try {
      const r = await gitApi.post<{ sha: string }>(pid, 'commit', { message: draft.message, amend: draft.amend, signoff: draft.signoff })
      toast('success', `Committed ${shortSha(r.sha)}`)
      useDrafts.getState().reset(pid)
    } catch (e) {
      toastError(e, 'Commit failed')
    } finally {
      setBusy(false)
    }
  }

  const total = s.files.filter((f) => f.index !== '!').length
  return (
    <div className="git-mobile">
      <div className="top">
        <GitBranch size={16} className="wb-muted" />
        <b className="wb-ellipsis">{s.branch ?? (s.head ? shortSha(s.head) : 'no commits')}</b>
        {(s.ahead > 0 || s.behind > 0) && (
          <span className="git-sync">
            {s.ahead > 0 && `↑${s.ahead} `}
            {s.behind > 0 && `↓${s.behind}`}
          </span>
        )}
        <span style={{ flex: 1 }} />
        <IconButton icon={CloudDownload} label="Fetch" onClick={() => void fetchAll(pid)} />
        <Button size="small" icon={ArrowDownToLine} onClick={() => void updateProject(pid)}>
          Pull
        </Button>
        <Button size="small" icon={ArrowUpFromLine} onClick={() => openPush(pid)}>
          Push
        </Button>
      </div>
      {s.state === 'bisecting' && <BisectBanner pid={pid} compact />}
      {s.state !== 'clean' && s.state !== 'bisecting' && (
        <div className="git-banner">
          <AlertTriangle size={14} className="wb-warning" />
          <span className="text">
            <b>{stateLabel(s.state)}</b>
            {sections.conflicts.length ? ` · ${sections.conflicts.length} conflict(s): resolve them on the desktop` : ''}
          </span>
          <Button size="small" disabled={sections.conflicts.length > 0} onClick={() => void sequencer(pid, 'continue', stateLabel(s.state))}>
            Continue
          </Button>
          <Button size="small" onClick={() => void sequencer(pid, 'abort', stateLabel(s.state))}>
            Abort
          </Button>
        </div>
      )}
      <div className="list">
        {total === 0 && <EmptyState icon={Check} title="No local changes" />}
        {(['conflicts', 'staged', 'unstaged', 'untracked'] as SectionId[]).map((sec) =>
          sections[sec].length ? (
            <div key={sec}>
              <div className="git-section">
                <span className={sec === 'conflicts' ? 'git-c-conflict' : undefined}>{TITLES[sec]}</span>
                <span className="count">{sections[sec].length}</span>
                {sec !== 'conflicts' && (
                  <span style={{ marginLeft: 'auto' }}>
                    <Button
                      size="small"
                      variant="ghost"
                      onClick={() =>
                        sec === 'staged' ? void gitApi.post(pid, 'unstage', { all: true }).catch((e) => toastError(e)) : void stagePaths(pid, sections[sec].map((f) => f.path))
                      }
                    >
                      {sec === 'staged' ? 'Unstage all' : 'Stage all'}
                    </Button>
                  </span>
                )}
              </div>
              {sections[sec].slice(0, 500).map((f) => {
                const code = sectionCode(f, sec)
                const { name, dir } = splitPath(f.path)
                return (
                  <div key={f.path} className="git-row" onClick={() => sec !== 'conflicts' && toggle(sec, f)}>
                    {sec === 'conflicts' ? (
                      <AlertTriangle size={16} className="git-c-conflict" />
                    ) : (
                      <span className={`git-toggle${sec === 'staged' ? ' on' : ''}`}>{sec === 'staged' && <Check size={14} />}</span>
                    )}
                    <FileText size={15} className="icon" />
                    <span className={`name ${statusClass(code)}`}>{name}</span>
                    <span className="dir">{dir}</span>
                  </div>
                )
              })}
            </div>
          ) : null,
        )}
      </div>
      <div className="commit">
        <TextArea value={draft.message} placeholder="Commit message" onChange={(e) => update(pid, { message: e.target.value })} aria-label="Commit message" />
        <div className="wb-row" style={{ gap: 12 }}>
          <Checkbox
            checked={draft.amend}
            onChange={async (v) => {
              update(pid, { amend: v })
              if (v && !draft.message.trim()) {
                const { message } = await gitApi.lastMessage(pid).catch(() => ({ message: '' }))
                update(pid, { message, amendLoaded: message })
              }
            }}
          >
            Amend
          </Checkbox>
          <IconButton icon={Sparkles} label="Ask agent for a commit message" onClick={() => void askCommitMessage(pid)} />
          <span style={{ flex: 1 }} />
          <Button variant="primary" loading={busy} disabled={!canCommit} onClick={() => void commit()}>
            Commit {sections.staged.length ? `(${sections.staged.length})` : ''}
          </Button>
        </div>
      </div>
    </div>
  )
}
