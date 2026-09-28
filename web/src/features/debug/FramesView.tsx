// Frames (CLion's "Threads & Variables" left pane): the thread picker and the call
// stack of the selected thread. Selecting a frame shows its source and its variables.

import { Layers } from 'lucide-react'
import { EmptyState, Select, Spinner } from '@/ui'
import { openFrame } from './actions'
import { frameLocation } from './logic'
import { useDebug } from './store'
import type { DebugSession } from './types'

export function FramesView({ s }: { s: DebugSession }) {
  const stack = useDebug((st) => st.stacks[s.id])
  const sel = useDebug((st) => st.selection[s.id])
  const select = useDebug((st) => st.select)
  if (s.state !== 'stopped') {
    return (
      <div className="wb-dbg-frames">
        <EmptyState icon={Layers} title={s.state === 'running' ? 'The program is running' : s.state === 'starting' ? 'Starting…' : 'Not suspended'}>
          {s.state === 'running' ? 'Frames show when it stops at a breakpoint, or when you pause it.' : null}
        </EmptyState>
      </div>
    )
  }
  const threadId = sel?.threadId ?? s.stopped?.threadId
  const frames = stack && stack.epoch === s.stopEpoch ? stack.frames : []
  const frameIndex = sel?.frameIndex ?? 0
  return (
    <div className="wb-dbg-frames">
      <div className="wb-dbg-threadbar">
        {s.threads.length > 1 ? (
          <Select value={String(threadId ?? '')} onChange={(e) => select(s.id, { threadId: Number(e.target.value), frameIndex: 0 })} aria-label="Thread">
            {s.threads.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name || `Thread ${t.id}`}
                {t.id === s.stopped?.threadId ? ` — ${s.stopped.reason}` : ''}
              </option>
            ))}
          </Select>
        ) : (
          <span className="wb-small wb-muted wb-ellipsis">
            Thread {s.threads[0]?.name || threadId || ''} {s.stopped ? `— ${s.stopped.reason}` : ''}
          </span>
        )}
      </div>
      <div className="wb-scroll wb-dbg-list" role="listbox" aria-label="Frames">
        {stack?.loading && (
          <div className="wb-dbg-note">
            <Spinner /> Loading frames…
          </div>
        )}
        {stack?.error && <div className="wb-dbg-note wb-danger">{stack.error}</div>}
        {frames.map((f, i) => {
          const noSource = !f.source?.path
          const outside = !!f.source?.path && !f.source.inProject
          return (
            <div
              key={`${f.id}:${i}`}
              role="option"
              aria-selected={i === frameIndex}
              className={['wb-list-row', 'wb-dbg-frame', i === frameIndex && 'selected', (noSource || f.presentationHint === 'subtle') && 'dim', outside && 'outside'].filter(Boolean).join(' ')}
              title={f.source?.path ? `${f.source.path}:${f.line}` : (f.source?.name ?? 'no source')}
              onClick={() => {
                select(s.id, { frameIndex: i })
                openFrame(s, f, false)
              }}
              onDoubleClick={() => openFrame(s, f, true)}
            >
              <span className="wb-dbg-fname wb-ellipsis">{f.name}</span>
              <span className="wb-dbg-floc wb-ellipsis">{frameLocation(f)}</span>
            </div>
          )
        })}
        {!stack?.loading && !stack?.error && frames.length === 0 && <div className="wb-dbg-note wb-muted">No frames.</div>}
      </div>
    </div>
  )
}
