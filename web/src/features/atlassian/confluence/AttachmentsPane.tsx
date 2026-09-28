// A page's attachments beside it: previews, upload (button or drop, with progress),
// download, delete to the trash. Uploading a name that exists asks before adding a
// new version of it.

import { useRef, useState, type DragEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Download, File, FileArchive, FileCode, FileSpreadsheet, FileText, Image as ImageIcon, Paperclip, RefreshCw, Trash2, Upload, X } from 'lucide-react'
import { ApiError } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { Button, EmptyState, ErrorBox, IconButton, Loading, Spacer, TimeAgo, Toolbar } from '@/ui'
import { confluenceApi, qk, useAttachments, type Attachment, type Page } from '../api'
import { fileSize } from '../links'
import { StaleNotice } from '../StaleNotice'
import { MAX_UPLOAD_BYTES } from './uploads'

interface Pending {
  id: number
  name: string
  sent: number
  total: number
  abort: AbortController
}

function TypeIcon({ a }: { a: Attachment }) {
  const t = a.mediaType
  const n = a.title.toLowerCase()
  if (t.startsWith('image/')) return <ImageIcon size={18} />
  if (t === 'application/pdf' || t.startsWith('text/')) return <FileText size={18} />
  if (/\.(zip|tar|gz|tgz|7z|rar)$/.test(n)) return <FileArchive size={18} />
  if (/\.(csv|xlsx?|ods)$/.test(n)) return <FileSpreadsheet size={18} />
  if (/\.(json|xml|ya?ml|toml|rs|ts|js|py|go|java|c|cpp|h|sh)$/.test(n)) return <FileCode size={18} />
  return <File size={18} />
}

let seq = 0

export function AttachmentsPane({ projectId, page, onClose }: { projectId: string | null; page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const q = useAttachments(projectId, page.id)
  const [pending, setPending] = useState<Pending[]>([])
  const [over, setOver] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  const readOnly = page.status !== 'current' || page.historical
  const list = q.data?.attachments ?? []
  const refresh = () => qc.invalidateQueries({ queryKey: qk.attachments(projectId, page.id) })

  const uploadOne = async (file: File) => {
    if (file.size > MAX_UPLOAD_BYTES) {
      toast('warning', `“${file.name}” is larger than ${MAX_UPLOAD_BYTES / 1024 / 1024} MB`)
      return
    }
    let replace = false
    if (list.some((a) => a.title === file.name)) {
      replace = await confirmDialog({
        title: `Replace “${file.name}”?`,
        message: 'An attachment with this name is already on the page. Uploading adds a new version of it; the old versions stay in its history.',
        confirmLabel: 'Upload new version',
      })
      if (!replace) return
    }
    const p: Pending = { id: ++seq, name: file.name, sent: 0, total: file.size, abort: new AbortController() }
    setPending((l) => [...l, p])
    try {
      await confluenceApi.upload(projectId, page.id, file, {
        name: file.name,
        replace,
        signal: p.abort.signal,
        onProgress: (sent, total) => setPending((l) => l.map((x) => (x.id === p.id ? { ...x, sent, total } : x))),
      })
      toast('success', replace ? `Uploaded a new version of “${file.name}”` : `Attached “${file.name}”`)
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') toast('info', `Upload of “${file.name}” cancelled`)
      else if (e instanceof ApiError && e.code === 'exists') toast('warning', e.message)
      else toastError(e, `Could not upload “${file.name}”`)
    } finally {
      setPending((l) => l.filter((x) => x.id !== p.id))
      void refresh()
    }
  }

  const uploadAll = async (files: FileList | File[] | null) => {
    for (const f of Array.from(files ?? [])) await uploadOne(f)
  }

  const remove = async (a: Attachment) => {
    const ok = await confirmDialog({
      title: `Move “${a.title}” to the trash?`,
      message: 'Links and images of it in the page stop working. Confluence keeps it in the space trash, where a space admin can restore it.',
      confirmLabel: 'Move to trash',
      danger: true,
    })
    if (!ok) return
    try {
      await confluenceApi.deleteAttachment(projectId, a.id)
      toast('success', `“${a.title}” moved to the trash`)
    } catch (e) {
      toastError(e, `Could not delete “${a.title}”`)
    } finally {
      void refresh()
    }
  }

  const onDrop = (e: DragEvent) => {
    if (!e.dataTransfer.files.length) return
    e.preventDefault()
    setOver(false)
    if (readOnly) return toast('warning', 'This page is read-only')
    void uploadAll(e.dataTransfer.files)
  }

  return (
    <div
      className={over ? 'cf-side cf-drop over' : 'cf-side cf-drop'}
      onDragOver={(e) => {
        if (!readOnly && [...e.dataTransfer.types].includes('Files')) {
          e.preventDefault()
          setOver(true)
        }
      }}
      onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setOver(false)}
      onDrop={onDrop}
    >
      <Toolbar>
        <Paperclip size={13} className="wb-muted" />
        <span className="wb-small">Attachments</span>
        <span className="wb-xs wb-subtle">{list.length || ''}</span>
        <Spacer />
        {!readOnly && <IconButton size="small" icon={Upload} label="Upload files…" onClick={() => input.current?.click()} />}
        <IconButton size="small" icon={RefreshCw} label="Refresh" onClick={() => void refresh()} />
        <IconButton size="small" icon={X} label="Close attachments" onClick={onClose} />
      </Toolbar>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          void uploadAll(e.target.files ? Array.from(e.target.files) : null)
          e.target.value = ''
        }}
      />
      {pending.map((p) => (
        <div key={p.id} className="cf-upload">
          <span className="wb-ellipsis wb-small">{p.name}</span>
          <span className="wb-xs wb-subtle">{p.total ? `${Math.round((p.sent / p.total) * 100)}%` : ''}</span>
          <IconButton size="small" icon={X} label="Cancel upload" onClick={() => p.abort.abort()} />
          <div className="bar">
            <div style={{ width: `${p.total ? (p.sent / p.total) * 100 : 0}%` }} />
          </div>
        </div>
      ))}
      {q.error && q.data && <StaleNotice what="the attachments" error={q.error} onRetry={() => void q.refetch()} />}
      <div className="cf-comments-list">
        {q.isLoading ? (
          <Loading label="Loading attachments…" />
        ) : q.error && !q.data ? (
          <ErrorBox error={q.error} onRetry={() => q.refetch()} />
        ) : !list.length ? (
          <EmptyState icon={Paperclip} title="No attachments">
            {readOnly ? null : (
              <>
                Drop files here, or{' '}
                <Button size="small" variant="ghost" onClick={() => input.current?.click()}>
                  choose files…
                </Button>
              </>
            )}
          </EmptyState>
        ) : (
          list.map((a) => (
            <div key={a.id} className="cf-att" title={a.comment || a.title}>
              <a className="thumb" href={a.downloadUrl} target="_blank" rel="noopener noreferrer" aria-label={`Open ${a.title}`}>
                {a.isImage ? <img src={a.downloadUrl} alt="" loading="lazy" /> : <TypeIcon a={a} />}
              </a>
              <div className="info">
                <a className="name wb-ellipsis" href={a.downloadUrl} target="_blank" rel="noopener noreferrer">
                  {a.title}
                </a>
                <div className="meta wb-ellipsis">
                  {fileSize(a.fileSize)}
                  {a.version > 1 ? ` · v${a.version}` : ''} · <TimeAgo time={a.createdAt} />
                  {a.authorName ? ` · ${a.authorName}` : ''}
                </div>
              </div>
              <div className="acts">
                <IconButton size="small" icon={Download} label="Download" onClick={() => window.open(`${a.downloadUrl}&download=1`, '_blank', 'noopener,noreferrer')} />
                {!readOnly && <IconButton size="small" icon={Trash2} label="Move to trash…" onClick={() => void remove(a)} />}
              </div>
            </div>
          ))
        )}
        {q.data?.truncated && <div className="wb-xs wb-subtle">Showing the first 500 attachments.</div>}
      </div>
      {!readOnly && <div className="cf-drop-hint">Drop files to attach them</div>}
    </div>
  )
}
