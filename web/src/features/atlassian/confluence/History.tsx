// Page history: the version list (lazily paged: pages can have hundreds of versions)
// and a diff of any two versions, as readable text or as storage XHTML.

import { useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { History as HistoryIcon, RotateCcw } from 'lucide-react'
import { ApiError } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { monacoThemeName } from '@/theme/palette'
import { Button, EmptyState, ErrorBox, Loading, MonacoDiffEditor, Spacer, Tabs, TimeAgo, Toolbar } from '@/ui'
import { confluenceApi, qk, useVersionBody, useVersions, type Page, type PageVersion } from '../api'
import { formatForDiff, storageToText } from '../storage/convert'

export function History({ projectId, page }: { projectId: string | null; page: Page }) {
  const qc = useQueryClient()
  const versions = useVersions(projectId, page.id, true)
  const latest = page.version.number
  const [newer, setNewer] = useState(latest)
  const [older, setOlder] = useState<number | null>(latest > 1 ? latest - 1 : null)
  const [view, setView] = useState<'text' | 'xhtml'>('text')
  const a = useVersionBody(projectId, page.id, older)
  const b = useVersionBody(projectId, page.id, newer)
  const list: PageVersion[] = versions.data?.pages.flatMap((p) => p.results) ?? []
  const newerInfo = list.find((v) => v.number === newer)

  const fmt = (s: string | undefined) => (s === undefined ? '' : view === 'text' ? storageToText(s) : formatForDiff(s))
  const original = useMemo(() => (older === null ? '' : fmt(a.data?.storage)), [a.data, older, view]) // eslint-disable-line react-hooks/exhaustive-deps
  const modified = useMemo(() => fmt(b.data?.storage), [b.data, view]) // eslint-disable-line react-hooks/exhaustive-deps

  const pick = (n: number, shift: boolean) => {
    if (shift && n < newer) setOlder(n)
    else if (shift && n > newer) {
      setOlder(newer)
      setNewer(n)
    } else {
      setNewer(n)
      setOlder(n > 1 ? n - 1 : null)
    }
  }

  const restore = async () => {
    if (!b.data) return
    const ok = await confirmDialog({
      title: `Restore version ${newer}?`,
      message: `The page gets a new version ${latest + 1} with the content of version ${newer}. Later versions stay in the history.`,
      confirmLabel: 'Restore',
    })
    if (!ok) return
    const body = { title: b.data.title, storage: b.data.storage, version: latest, message: `Restored version ${newer}` }
    try {
      await confluenceApi.update(projectId, page.id, body)
    } catch (e) {
      if (e instanceof ApiError && e.code === 'inline_comments') {
        const force = await confirmDialog({ title: 'Inline comments would lose their place', message: e.message, confirmLabel: 'Restore anyway', danger: true })
        if (!force) return
        try {
          await confluenceApi.update(projectId, page.id, { ...body, force: true })
        } catch (e2) {
          toastError(e2, 'Restore failed')
          return
        }
      } else {
        toastError(e, 'Restore failed')
        return
      }
    }
    toast('success', `Restored version ${newer} as version ${latest + 1}`)
    qc.invalidateQueries({ queryKey: qk.page(projectId, page.id) })
    qc.invalidateQueries({ queryKey: qk.versions(projectId, page.id) })
  }

  return (
    <div className="cf-history">
      <div className="cf-history-list">
        <Toolbar title="Versions">
          <Spacer />
          <span className="wb-xs wb-subtle">Shift-click: compare from</span>
        </Toolbar>
        {versions.isLoading && <Loading />}
        {versions.error && <ErrorBox error={versions.error} onRetry={() => versions.refetch()} />}
        {list.map((v) => (
          <div
            key={v.number}
            className={['cf-ver', v.number === newer && 'sel-new', v.number === older && 'sel-old'].filter(Boolean).join(' ')}
            onClick={(e) => pick(v.number, e.shiftKey)}
            title={v.message || undefined}
          >
            <span className="n">v{v.number}</span>
            <span className="msg">{v.message || <span className="wb-subtle">No comment</span>}</span>
            <span className="who">
              {v.authorName ?? 'Someone'} · <TimeAgo time={v.createdAt} />
              {v.minorEdit ? ' · minor' : ''}
            </span>
          </div>
        ))}
        {versions.hasNextPage && (
          <div className="wb-pad">
            <Button size="small" loading={versions.isFetchingNextPage} onClick={() => versions.fetchNextPage()}>
              Older versions
            </Button>
          </div>
        )}
      </div>
      <div className="cf-history-diff">
        <Toolbar
          title={
            older === null ? (
              <>Version {newer} (first)</>
            ) : (
              <>
                v{older} → v{newer}
              </>
            )
          }
        >
          {newerInfo && (
            <span className="wb-small wb-muted wb-ellipsis" style={{ marginLeft: 6 }}>
              {newerInfo.authorName ?? 'Someone'} · <TimeAgo time={newerInfo.createdAt} />
              {newerInfo.message ? ` · ${newerInfo.message}` : ''}
            </span>
          )}
          <Spacer />
          <Tabs
            tabs={[
              { id: 'text', label: 'Text' },
              { id: 'xhtml', label: 'Storage' },
            ]}
            value={view}
            onChange={setView}
          />
          {newer !== latest && (
            <Button size="small" icon={RotateCcw} disabled={!b.data} onClick={() => void restore()} style={{ marginLeft: 6 }}>
              Restore v{newer}
            </Button>
          )}
        </Toolbar>
        <div style={{ flex: 1, minHeight: 0 }}>
          {a.error || b.error ? (
            <ErrorBox error={a.error ?? b.error} />
          ) : (older !== null && a.isLoading) || b.isLoading ? (
            <Loading label="Loading versions…" />
          ) : b.data ? (
            <MonacoDiffEditor
              height="100%"
              theme={monacoThemeName()}
              language={view === 'xhtml' ? 'xml' : 'plaintext'}
              original={original}
              modified={modified}
              // Two models per page, reused across selections and kept on unmount: letting the
              // wrapper dispose them races the diff widget ("TextModel got disposed…").
              originalModelPath={`confluence-diff://${page.id}/history-original`}
              modifiedModelPath={`confluence-diff://${page.id}/history-modified`}
              keepCurrentOriginalModel
              keepCurrentModifiedModel
              options={{ readOnly: true, originalEditable: false, renderSideBySide: true, wordWrap: 'on', diffWordWrap: 'on', automaticLayout: true, minimap: { enabled: false }, scrollBeyondLastLine: false, fontSize: 12 }}
            />
          ) : (
            <EmptyState icon={HistoryIcon} title="Pick a version" />
          )}
        </div>
      </div>
    </div>
  )
}
