// The `httpResponse` panel: the HTTP Client's responses for a project, newest first.
// Status, time and size; the body (JSON pretty-printed), the headers, and the
// request as sent (private env values masked).

import { useMemo, useState } from 'react'
import { ExternalLink, Globe, RotateCw } from 'lucide-react'
import type { PanelProps } from '@/shell/types'
import { openPanel } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Button, EmptyState, ErrorBox, MonacoEditor, Select, Spinner, Tabs, formatBytes } from '@/ui'
import type { HttpResult } from './api'
import { sendAgain } from './editor'
import { useHttp, type HttpRun } from './store'

export interface HttpResponseParams {
  projectId: string
}

type Tab = 'body' | 'headers' | 'request'

function statusTone(status: number): string {
  if (status >= 500) return 'danger'
  if (status >= 400) return 'warning'
  if (status >= 300) return 'accent'
  return 'success'
}

/** Pretty JSON when it parses; the text as it came otherwise. */
export function prettyBody(r: Pick<HttpResult, 'body' | 'contentType'>): { text: string; language: string } {
  const ct = (r.contentType ?? '').toLowerCase()
  const looksJson = ct.includes('json') || /^\s*[[{]/.test(r.body)
  if (looksJson) {
    try {
      return { text: JSON.stringify(JSON.parse(r.body), null, 2), language: 'json' }
    } catch {
      /* not JSON after all */
    }
  }
  if (ct.includes('html')) return { text: r.body, language: 'html' }
  if (ct.includes('xml')) return { text: r.body, language: 'xml' }
  if (ct.includes('javascript')) return { text: r.body, language: 'javascript' }
  if (ct.includes('css')) return { text: r.body, language: 'css' }
  return { text: r.body, language: 'plaintext' }
}

function runLabel(run: HttpRun): string {
  const r = run.result
  const p = run.pending
  const time = new Date(r?.at ?? p?.at ?? Date.now()).toLocaleTimeString()
  if (r) return `${r.request.method} ${r.name ?? shortUrl(r.request.url)} · ${r.status} · ${time}`
  return `${p?.method ?? ''} ${p?.path ?? ''}:${p?.line ?? ''} · ${run.error ? 'failed' : 'sending…'} · ${time}`
}

function shortUrl(u: string): string {
  try {
    const x = new URL(u)
    return `${x.pathname}${x.search}`
  } catch {
    return u
  }
}

export function HttpResponsePanel({ params }: PanelProps<HttpResponseParams>) {
  const all = useHttp((s) => s.runs)
  const selected = useHttp((s) => s.selected)
  const runs = useMemo(() => all.filter((r) => r.projectId === params.projectId), [all, params.projectId])
  const run = runs.find((r) => r.id === selected) ?? runs[0]
  const [tab, setTab] = useState<Tab>('body')
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)

  if (!run) {
    return (
      <EmptyState icon={Globe} title="No requests sent yet">
        Open an .http file and click ▶ Send Request above a request, or press Ctrl+Enter in it.
      </EmptyState>
    )
  }
  const r = run.result
  const where = r ? { path: r.path, line: r.line } : run.pending ? { path: run.pending.path, line: run.pending.line } : null
  const body = r ? prettyBody(r) : null

  return (
    <div className="wb-fill wb-http">
      <div className="wb-http-bar">
        <Select className="wb-http-runs" value={String(run.id)} onChange={(e) => useHttp.setState({ selected: Number(e.target.value) })} aria-label="Response">
          {runs.map((x) => (
            <option key={x.id} value={x.id}>
              {runLabel(x)}
            </option>
          ))}
        </Select>
        <span className="wb-grow" />
        {where && (
          <>
            <Button size="small" icon={RotateCw} onClick={() => void sendAgain(params.projectId, where.path, where.line)}>
              Send Again
            </Button>
            <Button
              size="small"
              icon={ExternalLink}
              onClick={() =>
                openPanel({ kind: 'editor', id: `editor:${params.projectId}:${where.path}`, params: { projectId: params.projectId, path: where.path, line: where.line, t: Date.now() } })
              }
            >
              Open Request
            </Button>
          </>
        )}
      </div>
      {!r && !run.error && (
        <div className="wb-http-pending">
          <Spinner size={12} /> Sending {run.pending?.method} request…
        </div>
      )}
      {run.error && <ErrorBox error={new Error(run.error)} />}
      {r && body && (
        <>
          <div className="wb-http-summary">
            <span className={`wb-http-status ${statusTone(r.status)}`}>
              {r.status} {r.statusText}
            </span>
            <span className="wb-http-url wb-ellipsis" title={r.finalUrl}>
              {r.request.method} {r.finalUrl}
            </span>
            <span className="wb-subtle wb-small">{r.elapsedMs} ms</span>
            <span className="wb-subtle wb-small">
              {formatBytes(r.size)}
              {r.truncated ? ' (first 5 MB)' : ''}
            </span>
            {r.env && <span className="wb-subtle wb-small">env {r.env}</span>}
          </div>
          <Tabs
            value={tab}
            onChange={setTab}
            tabs={[
              { id: 'body', label: 'Body' },
              { id: 'headers', label: `Headers (${r.headers.length})` },
              { id: 'request', label: 'Request' },
            ]}
          />
          <div className="wb-http-content">
            {tab === 'body' &&
              (r.body ? (
                <MonacoEditor
                  theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'}
                  language={body.language}
                  value={body.text}
                  path={`inmemory://http-response/${run.id}`}
                  options={{ readOnly: true, minimap: { enabled: false }, fontSize, scrollBeyondLastLine: false, automaticLayout: true, wordWrap: 'on', glyphMargin: false }}
                />
              ) : (
                <EmptyState icon={Globe} title="No body" />
              ))}
            {tab === 'headers' && <HeaderTable headers={r.headers} />}
            {tab === 'request' && (
              <div className="wb-scroll wb-http-request">
                <div className="wb-http-reqline">
                  {r.request.method} {r.request.url}
                </div>
                <HeaderTable headers={r.request.headers} />
                {r.request.body !== null && <pre className="wb-http-reqbody">{r.request.body}</pre>}
                <p className="wb-small wb-subtle">Values from http-client.private.env.json are shown as ••••.</p>
              </div>
            )}
          </div>
        </>
      )}
    </div>
  )
}

function HeaderTable({ headers }: { headers: [string, string][] }) {
  if (!headers.length) return <div className="wb-subtle wb-small wb-http-none">No headers</div>
  return (
    <table className="wb-http-headers">
      <tbody>
        {headers.map(([k, v], i) => (
          <tr key={`${k}:${i}`}>
            <th>{k}</th>
            <td>{v}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
