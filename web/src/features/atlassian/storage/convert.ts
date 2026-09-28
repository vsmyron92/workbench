// Confluence storage XHTML <-> ProseMirror JSON for the TipTap editor.
//
// Why JSON rather than HTML: HTML parsing loses CDATA (code macros) and namespaced
// elements, and TipTap drops whatever its schema does not know. Here every storage
// element maps to a node the schema (./schema.ts) knows:
//  * plain XHTML (p, h1-6, lists, tables, pre, blockquote, hr, br, marks) → editable nodes,
//    with extra attributes kept in `xattrs` and written back unchanged;
//  * the code macro → an editable code block that rebuilds the macro (other parameters kept);
//  * macros with a rich-text body (info, note, panel, expand…) → an editable container
//    whose macro XML is kept as a template;
//  * page layouts → editable sections and cells;
//  * inline-comment markers → a mark, so the text stays editable and the marker survives;
//  * everything else (links to pages, mentions, images, status lozenges, task lists,
//    unknown elements) → opaque atoms holding the original XML byte-for-byte.
// Paragraphs synthesized for bare text (`<li>text</li>`) are flagged so they are written
// back without `<p>`.

import { attr, childEl, escAttr, escText, parseXml, textOf, type XElement, type XNode } from './xml'

export interface PMMark {
  type: string
  attrs?: Record<string, unknown>
}

export interface PMNode {
  type: string
  attrs?: Record<string, unknown>
  content?: PMNode[]
  marks?: PMMark[]
  text?: string
}

export type XAttrs = [string, string][] | null

export interface ConvertOptions {
  /** The page being edited (image previews of its attachments). */
  pageId?: string
  /** Display names of mentioned accounts (accountId → name), for mention chips. */
  users?: Record<string, string>
}

/** Code macro details kept on a code block so the macro is rebuilt faithfully. */
export interface CodeMacro {
  kind: 'macro' | 'pre'
  /** Macro XML with U+0001 where the language parameter goes and U+0002 for the body. */
  tpl?: string
  langRaw?: string | null
  lang?: string | null
  bodyRaw?: string
  text?: string
  xattrs?: XAttrs
}

const BLOCK_HTML = new Set([
  'p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ul', 'ol', 'li', 'table', 'thead', 'tbody', 'tfoot', 'tr', 'td', 'th', 'pre',
  'blockquote', 'hr', 'div', 'colgroup', 'col', 'section', 'dl', 'dt', 'dd', 'figure', 'caption', 'center',
])
const INLINE_MACROS = new Set(['status', 'jira', 'anchor', 'mention', 'profile-picture', 'emoji', 'date'])
const INLINE_AC = new Set(['ac:link', 'ac:emoticon', 'ac:inline-comment-marker', 'ac:placeholder', 'ac:image'])
const MARK_TAGS: Record<string, string> = {
  strong: 'bold', b: 'bold', em: 'italic', i: 'italic', u: 'underline', s: 'strike', del: 'strike', strike: 'strike',
  code: 'code', sub: 'cfSub', sup: 'cfSup',
}
/** Serialization nesting, outermost first. */
// Matches real pages: <a><strong>, <strong><code>, <em><span style>.
const MARK_ORDER = ['link', 'cfComment', 'italic', 'bold', 'underline', 'strike', 'cfSub', 'cfSup', 'cfSpan', 'code']

interface Ctx {
  src: string
  opts: ConvertOptions
}

const raw = (ctx: Ctx, n: XNode) => ctx.src.slice(n.start, n.end)
const xa = (el: XElement, skip: string[] = []): XAttrs => {
  const a = el.attrs.filter(([k]) => !skip.includes(k))
  return a.length ? a : null
}
const isWs = (n: XNode) => n.type === 'text' && !n.cdata && !n.text.trim()

function isBlock(n: XNode): boolean {
  if (n.type === 'text') return false
  if (BLOCK_HTML.has(n.name)) return true
  if (n.name === 'ac:structured-macro') return !INLINE_MACROS.has(attr(n, 'ac:name') ?? '')
  if (INLINE_AC.has(n.name) || n.name.startsWith('ri:') || n.name === 'time') return false
  return n.name.startsWith('ac:')
}

// ---------------------------------------------------------------- storage → doc

/** Parse storage into ProseMirror JSON. Throws `XmlError` for malformed storage. */
export function storageToDoc(storage: string, opts: ConvertOptions = {}): PMNode {
  const ctx: Ctx = { src: storage, opts }
  const content = blocks(parseXml(storage), ctx)
  return { type: 'doc', content: content.length ? content : [leadPara()] }
}

function leadPara(): PMNode {
  return { type: 'paragraph', attrs: { synth: 'lead' } }
}

function withContent(node: PMNode, content: PMNode[]): PMNode {
  if (content.length) node.content = content
  return node
}

function blocks(nodes: XNode[], ctx: Ctx): PMNode[] {
  const out: PMNode[] = []
  let run: XNode[] = []
  const flush = () => {
    if (run.some((n) => !isWs(n))) out.push(withContent({ type: 'paragraph', attrs: { synth: 'wrap' } }, inlines(run, [], ctx)))
    run = []
  }
  for (const n of nodes) {
    if (isBlock(n)) {
      flush()
      out.push(...block(n as XElement, ctx))
    } else run.push(n)
  }
  flush()
  return out
}

/** Block content that must not be empty (`block+`). */
function blocksNonEmpty(nodes: XNode[], ctx: Ctx): PMNode[] {
  const b = blocks(nodes, ctx)
  return b.length ? b : [leadPara()]
}

function block(el: XElement, ctx: Ctx): PMNode[] {
  switch (el.name) {
    case 'p':
      return [withContent({ type: 'paragraph', attrs: { xattrs: xa(el) } }, inlines(el.children, [], ctx))]
    case 'h1':
    case 'h2':
    case 'h3':
    case 'h4':
    case 'h5':
    case 'h6':
      return [withContent({ type: 'heading', attrs: { level: Number(el.name[1]), xattrs: xa(el) } }, inlines(el.children, [], ctx))]
    case 'ul':
    case 'ol': {
      const items = listItems(el, ctx)
      if (!items) return [atom(el, ctx, false)]
      if (el.name === 'ul') return [{ type: 'bulletList', attrs: { xattrs: xa(el) }, content: items }]
      const start = parseInt(attr(el, 'start') ?? '1', 10)
      return [{ type: 'orderedList', attrs: { start: Number.isFinite(start) ? start : 1, xattrs: xa(el, ['start']) }, content: items }]
    }
    case 'blockquote':
      return [{ type: 'blockquote', attrs: { xattrs: xa(el) }, content: blocksNonEmpty(el.children, ctx) }]
    case 'pre': {
      if (el.children.some((c) => c.type === 'el')) return [atom(el, ctx, false)]
      const text = textOf(el)
      const cf: CodeMacro = { kind: 'pre', xattrs: xa(el) }
      return [withContent({ type: 'codeBlock', attrs: { language: null, cf } }, text ? [{ type: 'text', text }] : [])]
    }
    case 'hr':
      return el.attrs.length ? [atom(el, ctx, false)] : [{ type: 'horizontalRule' }]
    case 'table':
      return [table(el, ctx) ?? atom(el, ctx, false)]
    case 'ac:structured-macro':
      return [macro(el, ctx) ?? atom(el, ctx, false)]
    case 'ac:layout':
      return [layout(el, ctx) ?? atom(el, ctx, false)]
    default:
      return [atom(el, ctx, false)]
  }
}

function listItems(el: XElement, ctx: Ctx): PMNode[] | null {
  const items: PMNode[] = []
  for (const c of el.children) {
    if (isWs(c)) continue
    if (c.type !== 'el' || c.name !== 'li') return null
    const content = blocks(c.children, ctx)
    if (content[0]?.type !== 'paragraph') content.unshift(leadPara())
    items.push({ type: 'listItem', attrs: { xattrs: xa(c) }, content })
  }
  return items.length ? items : null
}

function table(el: XElement, ctx: Ctx): PMNode | null {
  const rows: PMNode[] = []
  let colgroup: string | null = null
  const row = (tr: XElement, section: string): PMNode | null => {
    const cells: PMNode[] = []
    for (const c of tr.children) {
      if (isWs(c)) continue
      if (c.type !== 'el' || (c.name !== 'td' && c.name !== 'th')) return null
      const colspan = parseInt(attr(c, 'colspan') ?? '1', 10) || 1
      const rowspan = parseInt(attr(c, 'rowspan') ?? '1', 10) || 1
      cells.push({
        type: c.name === 'th' ? 'tableHeader' : 'tableCell',
        attrs: { colspan, rowspan, colwidth: null, xattrs: xa(c, ['colspan', 'rowspan']) },
        content: blocksNonEmpty(c.children, ctx),
      })
    }
    return cells.length ? { type: 'tableRow', attrs: { xattrs: xa(tr), section }, content: cells } : null
  }
  for (const c of el.children) {
    if (isWs(c)) continue
    if (c.type !== 'el') return null
    if (c.name === 'colgroup') colgroup = raw(ctx, c)
    else if (c.name === 'tr') {
      const r = row(c, 'none')
      if (!r) return null
      rows.push(r)
    } else if (c.name === 'tbody' || c.name === 'thead' || c.name === 'tfoot') {
      for (const tr of c.children) {
        if (isWs(tr)) continue
        if (tr.type !== 'el' || tr.name !== 'tr') return null
        const r = row(tr, c.name)
        if (!r) return null
        rows.push(r)
      }
    } else return null
  }
  if (!rows.length) return null
  return { type: 'table', attrs: { xattrs: xa(el), cf: colgroup ? { colgroup } : null }, content: rows }
}

function macro(el: XElement, ctx: Ctx): PMNode | null {
  const name = attr(el, 'ac:name') ?? ''
  const body = childEl(el, 'ac:rich-text-body')
  const plain = childEl(el, 'ac:plain-text-body')
  const others = el.children.filter((c) => !isWs(c) && !(c.type === 'el' && (c.name === 'ac:parameter' || c === body || c === plain)))
  if (others.length) return null
  const rel = (n: XNode) => [n.start - el.start, n.end - el.start] as const
  const whole = raw(ctx, el)
  if (name === 'code' && plain && !body) {
    const langEl = el.children.find(
      (c): c is XElement => c.type === 'el' && c.name === 'ac:parameter' && attr(c, 'ac:name') === 'language',
    )
    const cuts: [number, number, string][] = [[...rel(plain), '\u0002']]
    if (langEl) cuts.push([...rel(langEl), '\u0001'])
    cuts.sort((a, b) => b[0] - a[0])
    let tpl = whole
    for (const [s, e, mark] of cuts) tpl = tpl.slice(0, s) + mark + tpl.slice(e)
    if (!langEl) tpl = tpl.replace('\u0002', '\u0001\u0002')
    const text = textOf(plain)
    const lang = langEl ? textOf(langEl).trim() || null : null
    const cf: CodeMacro = { kind: 'macro', tpl, langRaw: langEl ? raw(ctx, langEl) : null, lang, bodyRaw: raw(ctx, plain), text }
    return withContent({ type: 'codeBlock', attrs: { language: lang, cf } }, text ? [{ type: 'text', text }] : [])
  }
  if (body && !plain) {
    const [s, e] = [body.innerStart - el.start, body.innerEnd - el.start]
    // Self-closing <ac:rich-text-body/> has no inner range to replace.
    if (body.innerStart === body.end) return null
    const tpl = whole.slice(0, s) + '\u0000' + whole.slice(e)
    const title = el.children.find(
      (c): c is XElement => c.type === 'el' && c.name === 'ac:parameter' && attr(c, 'ac:name') === 'title',
    )
    return {
      type: 'cfRich',
      attrs: { tpl, name, title: title ? textOf(title) : '' },
      content: blocksNonEmpty(body.children, ctx),
    }
  }
  return null
}

function layout(el: XElement, ctx: Ctx): PMNode | null {
  const sections: PMNode[] = []
  for (const s of el.children) {
    if (isWs(s)) continue
    if (s.type !== 'el' || s.name !== 'ac:layout-section') return null
    const cells: PMNode[] = []
    for (const c of s.children) {
      if (isWs(c)) continue
      if (c.type !== 'el' || c.name !== 'ac:layout-cell') return null
      cells.push({ type: 'cfCell', attrs: { xattrs: xa(c) }, content: blocksNonEmpty(c.children, ctx) })
    }
    if (!cells.length) return null
    sections.push({ type: 'cfSection', attrs: { xattrs: xa(s) }, content: cells })
  }
  if (!sections.length) return null
  return { type: 'cfLayout', attrs: { xattrs: xa(el) }, content: sections }
}

/** What an opaque element is, for its chip in the editor. */
export function describe(el: XElement, opts: ConvertOptions = {}): { kind: string; label: string; preview: string | null } {
  const param = (name: string) => {
    const p = el.children.find((c): c is XElement => c.type === 'el' && c.name === 'ac:parameter' && attr(c, 'ac:name') === name)
    return p ? textOf(p).trim() : ''
  }
  switch (el.name) {
    case 'ac:structured-macro': {
      const name = attr(el, 'ac:name') ?? 'macro'
      if (name === 'status') return { kind: 'status', label: (param('title') || 'status').toUpperCase(), preview: param('colour').toLowerCase() || null }
      if (name === 'jira') return { kind: 'jira', label: param('key') || 'Jira issues', preview: null }
      if (name === 'toc') return { kind: 'macro', label: 'Table of contents', preview: null }
      if (name === 'children') return { kind: 'macro', label: 'Child pages', preview: null }
      if (name === 'anchor') return { kind: 'anchor', label: `#${param('') || textOf(el).trim()}`, preview: null }
      const title = param('title')
      return { kind: 'macro', label: title ? `${name}: ${title}` : name, preview: null }
    }
    case 'ac:link': {
      const body = childEl(el, 'ac:link-body') ?? childEl(el, 'ac:plain-text-link-body')
      const text = body ? textOf(body).trim() : ''
      const user = childEl(el, 'ri:user')
      if (user) {
        const name = opts.users?.[attr(user, 'ri:account-id') ?? '']
        return { kind: 'mention', label: text || (name ? `@${name}` : '@mention'), preview: null }
      }
      const page = childEl(el, 'ri:page')
      if (page) return { kind: 'link', label: text || attr(page, 'ri:content-title') || 'page', preview: null }
      const att = childEl(el, 'ri:attachment')
      if (att) return { kind: 'link', label: text || attr(att, 'ri:filename') || 'attachment', preview: null }
      const anchor = attr(el, 'ac:anchor')
      return { kind: 'link', label: text || (anchor ? `#${anchor}` : 'link'), preview: null }
    }
    case 'ac:image': {
      const att = childEl(el, 'ri:attachment')
      const url = childEl(el, 'ri:url')
      const file = att ? attr(att, 'ri:filename') : undefined
      let preview: string | null = null
      if (file && opts.pageId && !childEl(att!, 'ri:page')) {
        preview = `/api/confluence/attachments/${encodeURIComponent(opts.pageId)}/by-name/${encodeURIComponent(file)}`
      } else if (url) {
        const v = attr(url, 'ri:value') ?? ''
        if (/^https?:\/\//i.test(v)) preview = v
      }
      return { kind: 'image', label: file ?? attr(url ?? el, 'ri:value') ?? 'image', preview }
    }
    case 'ac:emoticon':
      return { kind: 'emoji', label: attr(el, 'ac:emoji-fallback') || `:${attr(el, 'ac:name') ?? 'emoji'}:`, preview: null }
    case 'ac:task-list': {
      const n = el.children.filter((c) => c.type === 'el' && c.name === 'ac:task').length
      return { kind: 'tasks', label: `Task list (${n})`, preview: null }
    }
    case 'time':
      return { kind: 'date', label: attr(el, 'datetime') ?? 'date', preview: null }
    case 'ac:layout':
      return { kind: 'layout', label: 'Page layout', preview: null }
    default: {
      const t = textOf(el).replace(/\s+/g, ' ').trim()
      return { kind: el.name, label: t.length > 80 ? t.slice(0, 80) + '…' : t || el.name, preview: null }
    }
  }
}

function atom(el: XElement, ctx: Ctx, inline: boolean, marks: PMMark[] = []): PMNode {
  const d = describe(el, ctx.opts)
  const node: PMNode = { type: inline ? 'cfInline' : 'cfBlock', attrs: { xml: raw(ctx, el), ...d } }
  if (inline && marks.length) node.marks = marks
  return node
}

/** Whitespace runs containing a newline render as one space; keep them that way. */
function normalizeWs(t: string): string {
  return t.replace(/[ \t\r\n]*\n[ \t\r\n]*/g, ' ')
}

function addMark(marks: PMMark[], m: PMMark): PMMark[] {
  const key = JSON.stringify(m)
  return marks.some((x) => JSON.stringify(x) === key) ? marks : [...marks, m]
}

function inlines(nodes: XNode[], marks: PMMark[], ctx: Ctx): PMNode[] {
  const out: PMNode[] = []
  for (const n of nodes) {
    if (n.type === 'text') {
      const text = n.cdata ? n.text : normalizeWs(n.text)
      if (text) out.push(marks.length ? { type: 'text', text, marks } : { type: 'text', text })
      continue
    }
    const markType = MARK_TAGS[n.name]
    if (markType) {
      // Formatting elements with attributes (styles, classes) are kept verbatim.
      if (n.attrs.length) out.push(atom(n, ctx, true, marks))
      else {
        const tagged = markType === 'bold' || markType === 'italic' || markType === 'strike'
        out.push(...inlines(n.children, addMark(marks, tagged ? { type: markType, attrs: { tag: n.name } } : { type: markType }), ctx))
      }
      continue
    }
    switch (n.name) {
      case 'a': {
        const href = attr(n, 'href')
        if (href === undefined) out.push(atom(n, ctx, true, marks))
        else out.push(...inlines(n.children, addMark(marks, { type: 'link', attrs: { href, xattrs: xa(n, ['href']) } }), ctx))
        break
      }
      case 'span':
        if (n.attrs.length) out.push(...inlines(n.children, addMark(marks, { type: 'cfSpan', attrs: { xattrs: xa(n) } }), ctx))
        else out.push(...inlines(n.children, marks, ctx))
        break
      case 'br':
        if (n.attrs.length) out.push(atom(n, ctx, true, marks))
        else out.push({ type: 'hardBreak' })
        break
      case 'ac:inline-comment-marker': {
        const ref = attr(n, 'ac:ref')
        if (ref === undefined || n.attrs.length !== 1) out.push(atom(n, ctx, true, marks))
        else out.push(...inlines(n.children, addMark(marks, { type: 'cfComment', attrs: { ref } }), ctx))
        break
      }
      default:
        out.push(atom(n, ctx, true, marks))
    }
  }
  return out
}

// ---------------------------------------------------------------- doc → storage

const attrsStr = (x: unknown): string =>
  Array.isArray(x) ? (x as [string, string][]).map(([k, v]) => ` ${k}="${escAttr(String(v))}"`).join('') : ''

const cdata = (text: string) => `<![CDATA[${text.replace(/]]>/g, ']]]]><![CDATA[>')}]]>`

/** ProseMirror JSON (as produced by storageToDoc or editor.getJSON()) → storage XHTML. */
export function docToStorage(doc: PMNode): string {
  return blocksOut(doc.content ?? [])
}

function blocksOut(nodes: PMNode[]): string {
  return nodes.map(blockOut).join('')
}

const textContent = (n: PMNode): string => (n.text ?? '') + (n.content ?? []).map(textContent).join('')

function blockOut(n: PMNode): string {
  const a = n.attrs ?? {}
  const xs = attrsStr(a.xattrs)
  switch (n.type) {
    case 'paragraph': {
      const inner = inlineOut(n.content ?? [])
      if (a.synth === 'wrap') return inner
      if (a.synth === 'lead' && !inner) return ''
      return inner ? `<p${xs}>${inner}</p>` : `<p${xs} />`
    }
    case 'heading': {
      const level = Math.min(6, Math.max(1, Number(a.level) || 1))
      return `<h${level}${xs}>${inlineOut(n.content ?? [])}</h${level}>`
    }
    case 'bulletList':
      return `<ul${xs}>${blocksOut(n.content ?? [])}</ul>`
    case 'orderedList': {
      const start = Number(a.start) || 1
      return `<ol${start !== 1 ? ` start="${start}"` : ''}${xs}>${blocksOut(n.content ?? [])}</ol>`
    }
    case 'listItem':
      return `<li${xs}>${blocksOut(n.content ?? [])}</li>`
    case 'blockquote':
      return `<blockquote${xs}>${blocksOut(n.content ?? [])}</blockquote>`
    case 'horizontalRule':
      return '<hr />'
    case 'codeBlock':
      return codeOut(n)
    case 'table':
      return tableOut(n)
    case 'cfBlock':
      return String(a.xml ?? '')
    case 'cfRich': {
      const body = blocksOut(n.content ?? [])
      return String(a.tpl ?? '').replace('\u0000', () => body)
    }
    case 'cfLayout':
      return `<ac:layout${xs}>${blocksOut(n.content ?? [])}</ac:layout>`
    case 'cfSection':
      return `<ac:layout-section${xs}>${blocksOut(n.content ?? [])}</ac:layout-section>`
    case 'cfCell':
      return `<ac:layout-cell${xs}>${blocksOut(n.content ?? [])}</ac:layout-cell>`
    default:
      // Unknown block (should not happen): keep its text rather than lose it.
      return `<p>${escText(textContent(n))}</p>`
  }
}

function codeOut(n: PMNode): string {
  const a = n.attrs ?? {}
  const cf = (a.cf ?? null) as CodeMacro | null
  const text = textContent(n)
  const lang = typeof a.language === 'string' && a.language.trim() ? a.language.trim() : null
  if (cf?.kind === 'pre' && !lang) return `<pre${attrsStr(cf.xattrs)}>${escText(text)}</pre>`
  const langXml = (l: string | null) => (l ? `<ac:parameter ac:name="language">${escText(l)}</ac:parameter>` : '')
  if (cf?.kind === 'macro' && cf.tpl) {
    const lx = lang === (cf.lang ?? null) && cf.langRaw ? cf.langRaw : langXml(lang)
    const bx = text === cf.text && cf.bodyRaw ? cf.bodyRaw : `<ac:plain-text-body>${cdata(text)}</ac:plain-text-body>`
    return cf.tpl.replace('\u0001', () => lx).replace('\u0002', () => bx)
  }
  return `<ac:structured-macro ac:name="code" ac:schema-version="1">${langXml(lang)}<ac:plain-text-body>${cdata(text)}</ac:plain-text-body></ac:structured-macro>`
}

function tableOut(n: PMNode): string {
  const a = n.attrs ?? {}
  const cf = (a.cf ?? null) as { colgroup?: string } | null
  let out = `<table${attrsStr(a.xattrs)}>${cf?.colgroup ?? ''}`
  let open: string | null = null
  let prev = 'tbody'
  for (const row of n.content ?? []) {
    const sec = typeof row.attrs?.section === 'string' ? (row.attrs.section as string) : prev
    prev = sec
    if (sec !== open) {
      if (open && open !== 'none') out += `</${open}>`
      if (sec !== 'none') out += `<${sec}>`
      open = sec
    }
    out += `<tr${attrsStr(row.attrs?.xattrs)}>`
    for (const cell of row.content ?? []) {
      const tag = cell.type === 'tableHeader' ? 'th' : 'td'
      const ca = cell.attrs ?? {}
      const span = (k: string) => (Number(ca[k]) > 1 ? ` ${k}="${Number(ca[k])}"` : '')
      out += `<${tag}${span('colspan')}${span('rowspan')}${attrsStr(ca.xattrs)}>${blocksOut(cell.content ?? [])}</${tag}>`
    }
    out += '</tr>'
  }
  if (open && open !== 'none') out += `</${open}>`
  return out + '</table>'
}

const markKey = (m: PMMark) => `${m.type}:${JSON.stringify(m.attrs ?? {})}`

function sortMarks(marks: PMMark[]): PMMark[] {
  const rank = (m: PMMark) => {
    const i = MARK_ORDER.indexOf(m.type)
    return i < 0 ? MARK_ORDER.length : i
  }
  return [...marks].sort((x, y) => rank(x) - rank(y))
}

function openMark(m: PMMark): string {
  const a = m.attrs ?? {}
  const tag = typeof a.tag === 'string' ? a.tag : null
  switch (m.type) {
    case 'link':
      return `<a href="${escAttr(String(a.href ?? ''))}"${attrsStr(a.xattrs)}>`
    case 'cfComment':
      return `<ac:inline-comment-marker ac:ref="${escAttr(String(a.ref ?? ''))}">`
    case 'cfSpan':
      return `<span${attrsStr(a.xattrs)}>`
    case 'bold':
      return `<${tag ?? 'strong'}>`
    case 'italic':
      return `<${tag ?? 'em'}>`
    case 'underline':
      return '<u>'
    case 'strike':
      return `<${tag ?? 's'}>`
    case 'cfSub':
      return '<sub>'
    case 'cfSup':
      return '<sup>'
    case 'code':
      return '<code>'
    default:
      return ''
  }
}

function closeMark(m: PMMark): string {
  const tag = typeof m.attrs?.tag === 'string' ? (m.attrs.tag as string) : null
  switch (m.type) {
    case 'link':
      return '</a>'
    case 'cfComment':
      return '</ac:inline-comment-marker>'
    case 'cfSpan':
      return '</span>'
    case 'bold':
      return `</${tag ?? 'strong'}>`
    case 'italic':
      return `</${tag ?? 'em'}>`
    case 'underline':
      return '</u>'
    case 'strike':
      return `</${tag ?? 's'}>`
    case 'cfSub':
      return '</sub>'
    case 'cfSup':
      return '</sup>'
    case 'code':
      return '</code>'
    default:
      return ''
  }
}

function inlineOut(nodes: PMNode[]): string {
  let out = ''
  const stack: PMMark[] = []
  for (const n of nodes) {
    const marks = n.type === 'hardBreak' ? stack.slice() : sortMarks(n.marks ?? [])
    let keep = 0
    while (keep < stack.length && keep < marks.length && markKey(stack[keep]) === markKey(marks[keep])) keep++
    while (stack.length > keep) out += closeMark(stack.pop()!)
    for (let i = keep; i < marks.length; i++) {
      out += openMark(marks[i])
      stack.push(marks[i])
    }
    if (n.type === 'text') out += escText(n.text ?? '')
    else if (n.type === 'hardBreak') out += '<br />'
    else if (n.type === 'cfInline') out += String(n.attrs?.xml ?? '')
    else out += escText(textContent(n))
  }
  while (stack.length) out += closeMark(stack.pop()!)
  return out
}

// ---------------------------------------------------------------- inserting storage elements

/** A user mention: `<ac:link><ri:user ri:account-id="…" /></ac:link>`. */
export function mentionXml(accountId: string): string {
  return `<ac:link><ri:user ri:account-id="${escAttr(accountId)}" /></ac:link>`
}

/**
 * A link to a page by title (and space, when it is not the page's own). `text` is the
 * link text when it differs from the title.
 */
export function pageLinkXml(title: string, spaceKey?: string | null, text?: string | null): string {
  const space = spaceKey ? ` ri:space-key="${escAttr(spaceKey)}"` : ''
  const body = text && text !== title ? `<ac:plain-text-link-body>${cdata(text)}</ac:plain-text-link-body>` : ''
  return `<ac:link><ri:page${space} ri:content-title="${escAttr(title)}" />${body}</ac:link>`
}

/** An attachment of this page: shown as an image, or linked. */
export function attachmentXml(filename: string, image: boolean): string {
  return image
    ? `<ac:image ac:alt="${escAttr(filename)}"><ri:attachment ri:filename="${escAttr(filename)}" /></ac:image>`
    : `<ac:link><ri:attachment ri:filename="${escAttr(filename)}" /></ac:link>`
}

/** An inline atom for storage XML built above (the editor node that keeps it verbatim). */
export function inlineAtom(xml: string, opts: ConvertOptions = {}): PMNode {
  const [el] = parseXml(xml)
  if (!el || el.type !== 'el') throw new Error('not an element')
  return { type: 'cfInline', attrs: { xml, ...describe(el, opts) } }
}

// ---------------------------------------------------------------- helpers for diffs and previews

const BREAK_AFTER =
  /(<\/(?:p|h[1-6]|li|tr|table|tbody|thead|ul|ol|blockquote|pre|ac:structured-macro|ac:layout-section|ac:layout-cell|ac:task|ac:rich-text-body)>|<(?:br|hr|p)\s*\/>|<(?:ul|ol|table|tbody|ac:layout-section|ac:rich-text-body)(?:\s[^>]*)?>)/g

/** Storage with line breaks after block boundaries, so line diffs are readable. */
export function formatForDiff(storage: string): string {
  return storage.replace(BREAK_AFTER, '$1\n').replace(/\n{2,}/g, '\n')
}

/** Plain text of storage (prose diffs, previews). Falls back to tag stripping. */
export function storageToText(storage: string): string {
  let nodes: XNode[]
  try {
    nodes = parseXml(storage)
  } catch {
    return storage.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ').trim()
  }
  const lines: string[] = []
  let cur = ''
  let prefix = ''
  const flush = () => {
    if (cur.trim()) {
      lines.push(prefix + cur.replace(/[ \t]+/g, ' ').trim())
      prefix = ''
    }
    cur = ''
  }
  const walk = (n: XNode) => {
    if (n.type === 'text') {
      cur += n.cdata ? n.text : n.text.replace(/\s+/g, ' ')
      return
    }
    if (n.name === 'ac:parameter') return
    if (n.name === 'br') {
      flush()
      return
    }
    const blockish = isBlock(n) || n.name === 'td' || n.name === 'th'
    if (blockish) flush()
    // A prefix applies to the next non-empty line (so <li><p>x</p></li> is "• x").
    if (n.name === 'td' || n.name === 'th') prefix = '| '
    if (n.name === 'li') prefix = '• '
    if (/^h[1-6]$/.test(n.name)) prefix = '#'.repeat(Number(n.name[1])) + ' '
    const d = n.name.startsWith('ac:') && !n.children.length ? describe(n).label : ''
    if (d) cur += `[${d}]`
    n.children.forEach(walk)
    if (blockish) flush()
  }
  nodes.forEach(walk)
  flush()
  return lines.join('\n')
}

/** Every inline-comment marker reference in a storage document or ProseMirror doc. */
export function markerRefs(storage: string): string[] {
  const out = new Set<string>()
  for (const m of storage.matchAll(/<ac:inline-comment-marker\b[^>]*?\bac:ref\s*=\s*(?:"([^"]*)"|'([^']*)')/g)) out.add(m[1] ?? m[2])
  return [...out]
}
