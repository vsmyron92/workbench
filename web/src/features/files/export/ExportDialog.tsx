// "Export as HTML…" (the Markdown editor's More menu, the preview panel, the
// palette): options, progress, then Download or Save next to the file.

import { useEffect, useRef, useState } from 'react'
import { create } from 'zustand'
import { Download, FileDown, FileText, Moon, Sun, TriangleAlert } from 'lucide-react'
import { ApiError } from '@/api/client'
import { confirmDialog, toast, toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Button, Checkbox, Field, Modal, Spinner } from '@/ui'
import { filesApi } from '../api'
import { bufferKey, getModel } from '../buffers'
import { revealInTree } from '../openers'
import { basename } from '../paths'
import { exportFileName, exportPath, IMAGE_CAP, TOTAL_IMAGE_CAP, type ExportTheme } from './document'

interface Target {
  projectId: string | null
  path: string
}

const useExportDialog = create<{ target: Target | null; set: (t: Target | null) => void }>()((set) => ({
  target: null,
  set: (target) => set({ target }),
}))

/** Open the export dialog for a Markdown file. */
export function exportAsHtml(projectId: string | null, path: string) {
  useExportDialog.getState().set({ projectId, path })
}

/** Mounted once by the files provider. */
export function ExportDialogHost() {
  const target = useExportDialog((s) => s.target)
  if (!target) return null
  return <ExportDialog key={`${target.projectId}:${target.path}`} {...target} onClose={() => useExportDialog.getState().set(null)} />
}

const MAX_WRITE = 60 * 1024 * 1024

/** The text to export: the open buffer (unsaved edits included), else the disk. */
async function sourceText(projectId: string | null, path: string): Promise<string> {
  const m = getModel(bufferKey(projectId, path))
  if (m && !m.isDisposed()) return m.getValue()
  const r = await filesApi.read(projectId, path)
  if (r.content === null) throw new Error(r.sensitive ? 'This file is marked sensitive: open it in the editor and reveal it first.' : 'Not a text file.')
  return r.content
}

function ExportDialog({ projectId, path, onClose }: Target & { onClose: () => void }) {
  const appTheme = useUi((s) => s.prefs.theme)
  const [theme, setTheme] = useState<ExportTheme>(appTheme === 'light' ? 'light' : 'dark')
  const [toc, setToc] = useState(true)
  const [embed, setEmbed] = useState(true)
  const [busy, setBusy] = useState<'download' | 'save' | null>(null)
  const [status, setStatus] = useState('')
  const [warnings, setWarnings] = useState<string[]>([])
  const abort = useRef<AbortController | null>(null)
  useEffect(() => () => abort.current?.abort(), [])
  const name = exportFileName(path)
  const target = exportPath(path)

  const run = async (how: 'download' | 'save') => {
    setBusy(how)
    setWarnings([])
    const ac = new AbortController()
    abort.current = ac
    try {
      const text = await sourceText(projectId, path)
      const { exportMarkdown } = await import('./render')
      const out = await exportMarkdown(projectId, path, text, { theme, toc, embedImages: embed }, setStatus, ac.signal)
      if (ac.signal.aborted) return
      if (how === 'download') {
        const url = URL.createObjectURL(new Blob([out.html], { type: 'text/html;charset=utf-8' }))
        const a = document.createElement('a')
        a.href = url
        a.download = name
        document.body.appendChild(a)
        a.click()
        a.remove()
        window.setTimeout(() => URL.revokeObjectURL(url), 30_000)
        toast('success', `Exported ${name}`, { detail: summary(out.embedded, out.warnings.length) })
      } else {
        if (!projectId) return
        if (out.html.length > MAX_WRITE) throw new Error('The export is too large to save into the project; download it instead.')
        setStatus('Saving…')
        if (!(await saveNextTo(projectId, target, out.html))) return
        toast('success', `Saved ${target}`, { detail: summary(out.embedded, out.warnings.length), action: { label: 'Reveal', run: () => revealInTree(projectId, target) } })
      }
      if (out.warnings.length) {
        setWarnings(out.warnings)
        setStatus('')
      } else onClose()
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return
      toastError(e, 'Export failed')
    } finally {
      if (abort.current === ac) abort.current = null
      setBusy(null)
      setStatus('')
    }
  }

  const close = () => {
    abort.current?.abort()
    onClose()
  }

  return (
    <Modal
      title="Export as HTML"
      onClose={close}
      footer={
        <>
          <Button onClick={close}>{busy ? 'Cancel' : warnings.length ? 'Close' : 'Cancel'}</Button>
          {projectId && (
            <Button icon={FileDown} loading={busy === 'save'} disabled={!!busy} onClick={() => void run('save')} title={`Write ${target} in the project`}>
              Save next to the file
            </Button>
          )}
          <Button variant="primary" icon={Download} loading={busy === 'download'} disabled={!!busy} onClick={() => void run('download')}>
            Download
          </Button>
        </>
      }
    >
      <div className="wb-export">
        <div className="wb-export-file">
          <FileText size={14} />
          <span className="wb-ellipsis" title={path}>
            {basename(path)}
          </span>
          <span className="wb-subtle">→ {name}</span>
        </div>
        <Field label="Theme">
          <span className="wb-lh-seg wb-export-seg" role="group" aria-label="Theme">
            <button className={theme === 'light' ? 'active' : ''} onClick={() => setTheme('light')} disabled={!!busy}>
              <Sun size={12} /> Light
            </button>
            <button className={theme === 'dark' ? 'active' : ''} onClick={() => setTheme('dark')} disabled={!!busy}>
              <Moon size={12} /> Dark
            </button>
          </span>
        </Field>
        <Checkbox checked={toc} onChange={setToc} disabled={!!busy}>
          Table of contents
        </Checkbox>
        <Checkbox checked={embed} onChange={setEmbed} disabled={!!busy}>
          Embed images
        </Checkbox>
        <div className="wb-export-hint">
          {embed
            ? `Images from the project go into the file (up to ${IMAGE_CAP / 1024 / 1024} MB each, ${TOTAL_IMAGE_CAP / 1024 / 1024} MB in all); larger ones keep their relative link.`
            : 'Images keep their relative links: they show when the file sits next to them.'}{' '}
          Diagrams are drawn in; the file runs no script.
        </div>
        {status && (
          <div className="wb-export-status">
            <Spinner /> {status}
          </div>
        )}
        {warnings.length > 0 && (
          <div className="wb-banner warning wb-export-warnings">
            <TriangleAlert size={14} />
            <div className="wb-grow">
              Exported, with {warnings.length} note{warnings.length === 1 ? '' : 's'}:
              <ul>
                {warnings.slice(0, 8).map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            </div>
          </div>
        )}
      </div>
    </Modal>
  )
}

function summary(embedded: number, notes: number): string {
  const parts = [embedded ? `${embedded} image${embedded === 1 ? '' : 's'} embedded` : 'no images embedded']
  if (notes) parts.push(`${notes} note${notes === 1 ? '' : 's'}`)
  return parts.join(' · ')
}

/** Write the export next to the source, asking before replacing a file. */
async function saveNextTo(projectId: string, target: string, html: string): Promise<boolean> {
  try {
    await filesApi.write(projectId, target, html, null)
    return true
  } catch (e) {
    if (!(e instanceof ApiError && e.code === 'conflict')) throw e
  }
  const ok = await confirmDialog({
    title: `Replace ${basename(target)}?`,
    message: `${target} already exists. Replace it with this export?`,
    confirmLabel: 'Replace',
    danger: true,
  })
  if (!ok) return false
  const s = await filesApi.stat(projectId, target)
  if (s.kind === 'dir') throw new Error(`${target} is a folder`)
  await filesApi.write(projectId, target, html, s.exists ? s.etag : null, !s.etag && s.exists)
  return true
}
