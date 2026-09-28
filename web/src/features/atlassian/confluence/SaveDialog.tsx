// Saving an edit: version comment, optional review diff, and the two refusals the
// server can answer with: `conflict` (someone saved a newer version: compare theirs
// with yours, then overwrite or reload) and `inline_comments` (the edit drops comment
// markers: confirm to drop them).

import { useState } from 'react'
import { AlertTriangle, GitCompare } from 'lucide-react'
import { ApiError } from '@/api/client'
import { monacoThemeName } from '@/theme/palette'
import { Button, Checkbox, ErrorBox, Field, Input, Loading, Modal, MonacoDiffEditor } from '@/ui'
import { confluenceApi, type Page, type UpdateOut } from '../api'
import { formatForDiff, markerRefs } from '../storage/convert'

export interface SaveRequest {
  base: number
  baseStorage: string
  title: string
  storage: string
}

type Stage = { kind: 'form' } | { kind: 'inline'; message: string; base: number } | { kind: 'conflict'; latest: Page | null; error?: unknown }

function Diff({ id, left, right, leftLabel, rightLabel }: { id: string; left: string; right: string; leftLabel: string; rightLabel: string }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <div className="wb-row wb-xs wb-muted">
        <span className="wb-grow">{leftLabel}</span>
        <span className="wb-grow">{rightLabel}</span>
      </div>
      <div style={{ height: '46vh', border: '1px solid var(--border)', borderRadius: 6, overflow: 'hidden' }}>
        <MonacoDiffEditor
          height="100%"
          theme={monacoThemeName()}
          language="xml"
          original={formatForDiff(left)}
          modified={formatForDiff(right)}
          // Reused per page and kept on unmount (see History.tsx).
          originalModelPath={`confluence-diff://${id}/save-original`}
          modifiedModelPath={`confluence-diff://${id}/save-modified`}
          keepCurrentOriginalModel
          keepCurrentModifiedModel
          options={{ readOnly: true, originalEditable: false, renderSideBySide: true, wordWrap: 'on', diffWordWrap: 'on', automaticLayout: true, minimap: { enabled: false }, scrollBeyondLastLine: false, fontSize: 12 }}
        />
      </div>
    </div>
  )
}

export function SaveDialog({
  projectId,
  page,
  req,
  onClose,
  onSaved,
  onDiscard,
}: {
  projectId: string | null
  page: Page
  req: SaveRequest
  onClose: () => void
  onSaved: (out: UpdateOut) => void
  onDiscard: () => void
}) {
  const [message, setMessage] = useState('')
  const [minor, setMinor] = useState(false)
  const [review, setReview] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const [stage, setStage] = useState<Stage>({ kind: 'form' })
  const lost = markerRefs(req.baseStorage).filter((r) => !markerRefs(req.storage).includes(r)).length

  const save = async (base: number, force = false) => {
    setBusy(true)
    setError(null)
    try {
      const out = await confluenceApi.update(projectId, page.id, {
        title: req.title,
        storage: req.storage,
        version: base,
        message: message.trim() || undefined,
        minorEdit: minor,
        force,
      })
      onSaved(out)
    } catch (e) {
      if (e instanceof ApiError && e.code === 'inline_comments') setStage({ kind: 'inline', message: e.message, base })
      else if (e instanceof ApiError && e.code === 'conflict') {
        setStage({ kind: 'conflict', latest: null })
        try {
          setStage({ kind: 'conflict', latest: await confluenceApi.page(projectId, page.id) })
        } catch (e2) {
          setStage({ kind: 'conflict', latest: null, error: e2 })
        }
      } else setError(e)
    } finally {
      setBusy(false)
    }
  }

  if (stage.kind === 'inline') {
    return (
      <Modal
        title="Inline comments would lose their place"
        onClose={onClose}
        footer={
          <>
            <Button onClick={() => setStage({ kind: 'form' })}>Back</Button>
            <Button variant="danger" loading={busy} onClick={() => void save(stage.base, true)}>
              Save anyway
            </Button>
          </>
        }
      >
        <div className="wb-row" style={{ alignItems: 'flex-start' }}>
          <AlertTriangle size={16} className="wb-warning" style={{ flex: 'none', marginTop: 2 }} />
          <div className="wb-small">{stage.message}</div>
        </div>
        <div className="wb-small wb-muted">
          Keep editing to restore the highlighted passages (Undo brings them back), or save anyway: the comments stay on the page but are no
          longer attached to text.
        </div>
        {error !== null && <ErrorBox error={error} />}
      </Modal>
    )
  }

  if (stage.kind === 'conflict') {
    const latest = stage.latest
    return (
      <Modal
        wide
        title="The page changed while you were editing"
        onClose={onClose}
        footer={
          <>
            <Button onClick={onClose}>Keep editing</Button>
            <Button onClick={onDiscard}>Discard mine and reload</Button>
            <Button variant="danger" loading={busy} disabled={!latest} onClick={() => latest && void save(latest.version.number)}>
              Overwrite with mine
            </Button>
          </>
        }
      >
        {stage.error !== undefined ? (
          <ErrorBox error={stage.error} />
        ) : !latest ? (
          <Loading label="Loading the latest version…" />
        ) : (
          <>
            <div className="wb-small">
              Version <b>{latest.version.number}</b> was saved by {latest.version.authorName ?? 'someone'} after you started from version{' '}
              <b>{req.base}</b>. Left: theirs. Right: yours. Overwriting replaces their changes with yours (both stay in the history).
            </div>
            <Diff id={page.id} left={latest.storage} right={req.storage} leftLabel={`Theirs · v${latest.version.number}`} rightLabel="Yours" />
          </>
        )}
        {error !== null && <ErrorBox error={error} />}
      </Modal>
    )
  }

  return (
    <Modal
      wide={review}
      title={`Save “${req.title}”`}
      onClose={onClose}
      footer={
        <>
          <Button icon={GitCompare} variant="ghost" onClick={() => setReview(!review)}>
            {review ? 'Hide changes' : 'Review changes'}
          </Button>
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} onClick={() => void save(req.base)}>
            Save as version {page.version.number + 1}
          </Button>
        </>
      }
    >
      <Field label="What changed? (version comment)">
        <Input
          autoFocus
          value={message}
          placeholder="e.g. Tightened the economy numbers"
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && !busy && void save(req.base)}
        />
      </Field>
      <Checkbox checked={minor} onChange={setMinor}>
        <span className="wb-small">Minor edit (don’t notify watchers)</span>
      </Checkbox>
      {lost > 0 && (
        <div className="wb-row wb-small wb-warning">
          <AlertTriangle size={14} /> This edit removes {lost} inline-comment marker{lost === 1 ? '' : 's'}.
        </div>
      )}
      {review && <Diff id={page.id} left={req.baseStorage} right={req.storage} leftLabel={`v${req.base}`} rightLabel="Your edit" />}
      {error !== null && <ErrorBox error={error} />}
    </Modal>
  )
}
