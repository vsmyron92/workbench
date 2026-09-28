// Bisect banner (Commit window, Git Log, phone): progress, the commit under test and
// Good / Bad / Skip / Reset; once found, the first bad commit with a link.

import { Crosshair, GitGraph } from 'lucide-react'
import { Button } from '@/ui'
import { useBisect, useCommitDetails } from './api'
import { markBisect, openCommit, openGitLog, resetBisect } from './actions'
import { bisectProgress, shortSha } from './logic'

export function BisectBanner({ pid, inLog = false, compact = false }: { pid: string; inLog?: boolean; compact?: boolean }) {
  const q = useBisect(pid)
  const s = q.data
  const focus = s?.active ? (s.result ?? s.current) : null
  const c = useCommitDetails(pid, focus)
  if (!s?.active) return null
  const subject = c.data?.subject
  return (
    <div className={`git-banner git-bisect${s.result ? ' found' : ''}${inLog ? '' : ' stacked'}`}>
      <Crosshair size={14} className={s.result ? 'wb-success' : 'wb-warning'} />
      <span className="text">
        {s.result ? (
          <>
            <b>First {s.termBad} commit:</b>{' '}
            <a className="git-link git-mono" onClick={() => openCommit(pid, s.result!)}>
              {shortSha(s.result)}
            </a>
            {subject && <> “{subject}”</>}
          </>
        ) : (
          <>
            <b>Bisecting</b> · {bisectProgress(s.remaining, s.steps)}
            {s.current && (
              <>
                {' '}· testing{' '}
                <a className="git-link git-mono" onClick={() => openCommit(pid, s.current!)}>
                  {shortSha(s.current)}
                </a>
                {subject && !compact && <> “{subject}”</>}
              </>
            )}
          </>
        )}
      </span>
      {!s.result && (
        <>
          <Button size="small" onClick={() => void markBisect(pid, 'good')} title={`The checked-out commit is ${s.termGood}`}>
            {cap(s.termGood)}
          </Button>
          <Button size="small" onClick={() => void markBisect(pid, 'bad')} title={`The checked-out commit is ${s.termBad}`}>
            {cap(s.termBad)}
          </Button>
          <Button size="small" onClick={() => void markBisect(pid, 'skip')} title="Cannot be tested: pick another commit nearby">
            Skip
          </Button>
        </>
      )}
      {!inLog && !compact && (
        <Button size="small" variant="ghost" icon={GitGraph} onClick={() => openGitLog(pid, { panel: true })}>
          Log
        </Button>
      )}
      <Button size="small" variant={s.result ? 'primary' : 'default'} onClick={() => void resetBisect(pid)} title={`Stop bisecting and go back to ${s.start ?? 'where you started'}`}>
        Reset
      </Button>
    </div>
  )
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)
