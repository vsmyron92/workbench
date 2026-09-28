// "Export as HTML…": render a Markdown file with the app's own renderer
// (ui/Markdown.tsx: sanitized, highlighted, alerts, Mermaid in strict mode) off
// screen, then turn that DOM into a standalone document: CSS from the app's
// stylesheets with the chosen theme's tokens as literal values, relative images
// as data URIs (fetched through the raw endpoint, capped), Mermaid diagrams as
// inline SVG in the chosen theme, in-document links pointing at the right ids,
// and a table of contents. Loaded on demand.

import { createRoot } from 'react-dom/client'
import { MarkdownAsync } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Markdown } from '@/ui'
import { findAnchor, splitFrontmatter } from '@/ui/markdownPlugins'
import { useUi } from '@/state/store'
import { filesApi } from '../api'
import { basename, isExternalHref, resolveLink, splitAnchor } from '../paths'
import { BASE_CSS, documentHtml, fetchEmbeddableImage, ImageSkip, resolveVars, tocHtml, varNames, type ExportTheme, type TocItem } from './document'

export interface ExportOptions {
  theme: ExportTheme
  toc: boolean
  embedImages: boolean
}

export interface ExportResult {
  html: string
  title: string
  warnings: string[]
  embedded: number
}

/** Stylesheet rules that style rendered Markdown. */
const WANTED = /\.wb-prose|\.wb-md-|\.hljs|\.wb-alert/
const RENDER_TIMEOUT = 20_000

function collectCss(): string {
  const out: string[] = []
  const visit = (rules: CSSRuleList): string[] => {
    const acc: string[] = []
    for (const r of Array.from(rules)) {
      if (r instanceof CSSStyleRule) {
        if (WANTED.test(r.selectorText)) acc.push(r.cssText)
      } else if (r instanceof CSSMediaRule || r instanceof CSSContainerRule || r instanceof CSSSupportsRule) {
        const inner = visit(r.cssRules)
        if (inner.length) {
          const at = r instanceof CSSMediaRule ? '@media' : r instanceof CSSContainerRule ? '@container' : '@supports'
          acc.push(`${at} ${r.conditionText} {\n${inner.join('\n')}\n}`)
        }
      }
    }
    return acc
  }
  for (const sheet of Array.from(document.styleSheets)) {
    try {
      out.push(...visit(sheet.cssRules))
    } catch {
      // A cross-origin sheet (fonts): not ours to copy.
    }
  }
  return out.join('\n')
}

/** Values of the theme's tokens `names` (read from an element that carries the theme). */
function themeValues(theme: ExportTheme, names: string[]): Map<string, string> {
  const probe = document.createElement('div')
  probe.setAttribute('data-theme', theme)
  probe.style.display = 'none'
  document.body.appendChild(probe)
  try {
    const style = getComputedStyle(probe)
    return new Map(names.map((n) => [n, style.getPropertyValue(n).trim()]))
  } finally {
    probe.remove()
  }
}

/** Mermaid sources in document order (the same parse the renderer does). */
async function mermaidSources(text: string): Promise<string[]> {
  const found: string[] = []
  const collect = () => (tree: { children?: unknown[] }) => {
    const walk = (n: { type?: string; lang?: string | null; value?: string; children?: unknown[] }) => {
      if (n.type === 'code' && n.lang === 'mermaid' && typeof n.value === 'string') found.push(n.value)
      for (const c of n.children ?? []) walk(c as typeof n)
    }
    walk(tree as never)
    tree.children = []
  }
  await MarkdownAsync({ children: splitFrontmatter(text).body, remarkPlugins: [remarkGfm, collect] })
  return found
}

function wait(ms: number) {
  return new Promise((r) => setTimeout(r, ms))
}

/** Render `text` off screen and return a detached copy of the result. */
async function renderOffscreen(text: string, resolveImage: (src: string) => string, signal?: AbortSignal): Promise<HTMLElement> {
  const host = document.createElement('div')
  host.setAttribute('aria-hidden', 'true')
  host.style.cssText = 'position:fixed;left:-20000px;top:0;width:860px;visibility:hidden;pointer-events:none;contain:strict;height:10px;overflow:hidden'
  document.body.appendChild(host)
  const root = createRoot(host)
  try {
    root.render(<Markdown text={text} resolveImage={resolveImage} />)
    const started = Date.now()
    for (;;) {
      if (signal?.aborted) throw new DOMException('Export cancelled', 'AbortError')
      const prose = host.querySelector('.wb-prose')
      const ready = prose && (prose.childElementCount > 0 || !text.trim())
      // Mermaid renders after mount; an empty diagram box is still working.
      const pending = host.querySelector('.wb-md-mermaid:empty')
      if (ready && !pending) return prose!.cloneNode(true) as HTMLElement
      if (Date.now() - started > RENDER_TIMEOUT) {
        if (prose) return prose.cloneNode(true) as HTMLElement
        throw new Error('The Markdown renderer did not finish in time')
      }
      await wait(50)
    }
  } finally {
    root.unmount()
    host.remove()
  }
}

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader()
    r.onload = () => resolve(String(r.result))
    r.onerror = () => reject(r.error ?? new Error('read failed'))
    r.readAsDataURL(blob)
  })
}

export async function exportMarkdown(
  projectId: string | null,
  path: string,
  text: string,
  opts: ExportOptions,
  progress: (message: string) => void,
  signal?: AbortSignal,
): Promise<ExportResult> {
  const warnings: string[] = []
  // Relative images load through the raw endpoint while rendering; remember what
  // the document said, to fall back to it (it works next to the file), and the
  // file it names.
  const originals = new Map<string, { src: string; resolved: string }>()
  const resolveImage = (src: string) => {
    if (!src || isExternalHref(src) || src.startsWith('data:')) return src
    const resolved = resolveLink(path, splitAnchor(src).path)
    if (resolved === null) return src
    const url = filesApi.rawUrl(projectId, resolved)
    originals.set(url, { src, resolved })
    return url
  }

  progress('Rendering…')
  const [sources, prose] = await Promise.all([mermaidSources(text), renderOffscreen(text, resolveImage, signal)])

  // Copy buttons need script; the export has none.
  prose.querySelectorAll('.wb-md-code-bar button').forEach((b) => b.remove())

  // In-document links point at the ids the renderer gave (headings `md-…`, footnotes `user-content-…`).
  prose.querySelectorAll<HTMLAnchorElement>('a[href^="#"]').forEach((a) => {
    const target = findAnchor(prose, a.getAttribute('href') ?? '')
    const id = target?.id || target?.getAttribute('name')
    if (id) a.setAttribute('href', `#${id}`)
  })

  // Mermaid in the export's theme (the app rendered it in its own).
  const appTheme = useUi.getState().prefs.theme === 'light' ? 'light' : 'dark'
  const slots = Array.from(prose.querySelectorAll<HTMLElement>('.wb-md-mermaid, .wb-md-code')).filter(
    (el) => el.classList.contains('wb-md-mermaid') || (el.querySelector('.wb-md-code-bar span')?.textContent ?? '').startsWith('mermaid ·'),
  )
  if (sources.length && opts.theme !== appTheme) {
    if (slots.length !== sources.length) {
      warnings.push('Diagrams keep the app’s colours (their sources could not be matched).')
    } else {
      progress('Drawing diagrams…')
      const mermaid = (await import('mermaid')).default
      mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: opts.theme === 'dark' ? 'dark' : 'default' })
      for (let i = 0; i < slots.length; i++) {
        if (!slots[i].classList.contains('wb-md-mermaid')) continue
        try {
          const out = await mermaid.render(`wb-export-mermaid-${i}-${Date.now().toString(36)}`, sources[i])
          slots[i].innerHTML = out.svg
        } catch {
          warnings.push(`Diagram ${i + 1} keeps the app’s colours.`)
        }
      }
    }
  }

  // Images: relative ones become data URIs (image files only, within the caps),
  // or keep their relative path (works when the file sits next to the document).
  // The Markdown may be a repository's (untrusted): `![](../.git/config)` must not
  // copy that file into the export.
  let embedded = 0
  let total = 0
  const imgs = Array.from(prose.querySelectorAll('img'))
  for (let i = 0; i < imgs.length; i++) {
    if (signal?.aborted) throw new DOMException('Export cancelled', 'AbortError')
    const img = imgs[i]
    img.removeAttribute('loading')
    const src = img.getAttribute('src') ?? ''
    const found = originals.get(src)
    if (found === undefined) continue
    const { src: original, resolved } = found
    if (!opts.embedImages) {
      img.setAttribute('src', original)
      continue
    }
    progress(`Embedding images ${i + 1}/${imgs.length}…`)
    try {
      const blob = await fetchEmbeddableImage(resolved, src, total, (u, init) => fetch(u, init), signal)
      img.setAttribute('src', await blobToDataUrl(blob))
      total += blob.size
      embedded++
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') throw e
      img.setAttribute('src', original)
      warnings.push(`${original}: ${e instanceof ImageSkip ? e.message : `could not be embedded (${e instanceof Error ? e.message : String(e)}), linked instead`}`)
    }
  }

  const headings = Array.from(prose.querySelectorAll<HTMLElement>('h1[id], h2[id], h3[id]'))
  const title = (prose.querySelector('h1')?.textContent ?? '').trim() || basename(path)
  const toc: TocItem[] = opts.toc
    ? headings.map((h) => ({ id: h.id, text: (h.textContent ?? '').trim(), level: Number(h.tagName.slice(1)) })).filter((h) => h.text)
    : []

  progress('Writing the document…')
  const raw = `:root { color-scheme: ${opts.theme}; }\n${BASE_CSS}\n${collectCss()}`
  const values = themeValues(opts.theme, varNames(raw))
  const css = resolveVars(raw, (n) => values.get(n))
  const html = documentHtml({ title, theme: opts.theme, css, body: prose.innerHTML, toc: tocHtml(toc), source: path, exportedAt: new Date() })
  return { html, title, warnings, embedded }
}
