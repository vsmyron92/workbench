// A small, strict XML reader for Confluence storage format. It keeps source offsets
// so macros can be preserved byte-for-byte, knows the HTML named entities storage
// uses (&nbsp; &rsquo; …, which XML parsers reject without a DTD), and reports
// errors with line and column. No DOM needed (runs in tests and workers).

import { ENTITIES } from './entities'

export interface XElement {
  type: 'el'
  name: string
  /** In source order; values decoded. */
  attrs: [string, string][]
  children: XNode[]
  /** Offsets into the source: the whole element, and its content. */
  start: number
  end: number
  innerStart: number
  innerEnd: number
}

export interface XText {
  type: 'text'
  text: string
  cdata: boolean
  start: number
  end: number
}

export type XNode = XElement | XText

export class XmlError extends Error {
  line: number
  column: number
  constructor(message: string, line: number, column: number) {
    super(`line ${line}, column ${column}: ${message}`)
    this.line = line
    this.column = column
  }
}

const MAX_DEPTH = 200
const NAME_CHAR = /[\p{L}\p{N}:_.-]/u

export function decodeEntity(name: string): string | null {
  if (name.startsWith('#')) {
    const n = name[1] === 'x' || name[1] === 'X' ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10)
    if (!Number.isFinite(n) || n <= 0 || n > 0x10ffff) return null
    return String.fromCodePoint(n)
  }
  const cp = ENTITIES[name]
  return cp === undefined ? null : String.fromCodePoint(cp)
}

class Parser {
  src: string
  pos = 0
  constructor(src: string) {
    this.src = src
  }

  err(at: number, message: string): XmlError {
    const before = this.src.slice(0, at)
    const line = before.split('\n').length
    const column = at - (before.lastIndexOf('\n') + 1) + 1
    return new XmlError(message, line, column)
  }

  decode(raw: string, at: number): string {
    if (!raw.includes('&')) return raw
    let out = ''
    let i = 0
    while (i < raw.length) {
      const amp = raw.indexOf('&', i)
      if (amp < 0) {
        out += raw.slice(i)
        break
      }
      out += raw.slice(i, amp)
      const semi = raw.indexOf(';', amp)
      if (semi < 0 || semi - amp > 32) throw this.err(at + amp, "'&' must start an entity like &amp;")
      const name = raw.slice(amp + 1, semi)
      const ch = decodeEntity(name)
      if (ch === null) throw this.err(at + amp, `unknown entity &${name};`)
      out += ch
      i = semi + 1
    }
    return out
  }

  nodes(depth: number, parent: { name: string; at: number } | null): XNode[] {
    if (depth > MAX_DEPTH) throw this.err(this.pos, 'elements are nested too deeply')
    const out: XNode[] = []
    const s = this.src
    for (;;) {
      if (this.pos >= s.length) {
        if (parent) throw this.err(parent.at, `<${parent.name}> is never closed`)
        return out
      }
      if (s.startsWith('<![CDATA[', this.pos)) {
        const end = s.indexOf(']]>', this.pos + 9)
        if (end < 0) throw this.err(this.pos, 'unterminated CDATA section')
        out.push({ type: 'text', text: s.slice(this.pos + 9, end), cdata: true, start: this.pos, end: end + 3 })
        this.pos = end + 3
      } else if (s.startsWith('<!--', this.pos)) {
        const end = s.indexOf('-->', this.pos + 4)
        if (end < 0) throw this.err(this.pos, 'unterminated comment')
        this.pos = end + 3
      } else if (s.startsWith('<?', this.pos)) {
        const end = s.indexOf('?>', this.pos)
        if (end < 0) throw this.err(this.pos, 'unterminated processing instruction')
        this.pos = end + 2
      } else if (s.startsWith('<!', this.pos)) {
        throw this.err(this.pos, 'DOCTYPE declarations are not allowed in storage format')
      } else if (s.startsWith('</', this.pos)) {
        const at = this.pos
        const end = s.indexOf('>', at)
        if (end < 0) throw this.err(at, 'unterminated end tag')
        const name = s.slice(at + 2, end).trim()
        if (!parent) throw this.err(at, `unexpected </${name}>`)
        if (parent.name !== name) throw this.err(at, `expected </${parent.name}> but found </${name}>`)
        this.pos = end + 1
        return out
      } else if (s[this.pos] === '<') {
        out.push(this.element(depth))
      } else {
        let end = s.indexOf('<', this.pos)
        if (end < 0) end = s.length
        const raw = s.slice(this.pos, end)
        out.push({ type: 'text', text: this.decode(raw, this.pos), cdata: false, start: this.pos, end })
        this.pos = end
      }
    }
  }

  element(depth: number): XElement {
    const s = this.src
    const start = this.pos
    let i = start + 1
    while (i < s.length && NAME_CHAR.test(s[i])) i++
    if (i === start + 1) throw this.err(start, "'<' must start a tag; write &lt; for a literal <")
    const name = s.slice(start + 1, i)
    const attrs: [string, string][] = []
    this.pos = i
    for (;;) {
      const ws = /^\s*/.exec(s.slice(this.pos, this.pos + 64))![0].length
      this.pos += ws
      if (s.startsWith('/>', this.pos)) {
        this.pos += 2
        return { type: 'el', name, attrs, children: [], start, end: this.pos, innerStart: this.pos, innerEnd: this.pos }
      }
      if (s[this.pos] === '>') {
        this.pos += 1
        const innerStart = this.pos
        const children = this.nodes(depth + 1, { name, at: start })
        // `nodes` consumed the end tag; find where it began.
        const innerEnd = s.lastIndexOf('</', this.pos - 1)
        return { type: 'el', name, attrs, children, start, end: this.pos, innerStart, innerEnd }
      }
      if (this.pos >= s.length) throw this.err(start, `<${name}> tag is not closed`)
      if (ws === 0) throw this.err(this.pos, 'expected whitespace between attributes')
      const at = this.pos
      let k = at
      while (k < s.length && NAME_CHAR.test(s[k])) k++
      if (k === at) throw this.err(at, `malformed attribute in <${name}>`)
      const key = s.slice(at, k)
      let p = k
      while (/\s/.test(s[p] ?? '')) p++
      if (s[p] !== '=') throw this.err(at, `attribute ${key} needs a quoted value`)
      p++
      while (/\s/.test(s[p] ?? '')) p++
      const q = s[p]
      if (q !== '"' && q !== "'") throw this.err(at, `attribute ${key} needs a quoted value`)
      const close = s.indexOf(q, p + 1)
      if (close < 0) throw this.err(at, `unterminated value of ${key}`)
      const raw = s.slice(p + 1, close)
      if (raw.includes('<')) throw this.err(at, `'<' is not allowed in the value of ${key}`)
      if (attrs.some(([k2]) => k2 === key)) throw this.err(at, `duplicate attribute ${key}`)
      attrs.push([key, this.decode(raw, p + 1)])
      this.pos = close + 1
    }
  }
}

/** Parse a storage-format fragment. Throws `XmlError`. */
export function parseXml(src: string): XNode[] {
  return new Parser(src).nodes(0, null)
}

export function attr(el: XElement, name: string): string | undefined {
  return el.attrs.find(([k]) => k === name)?.[1]
}

export function childEl(el: XElement, name: string): XElement | undefined {
  return el.children.find((c): c is XElement => c.type === 'el' && c.name === name)
}

export function textOf(n: XNode): string {
  return n.type === 'text' ? n.text : n.children.map(textOf).join('')
}

/** Escape text content for XHTML. Non-breaking spaces become `&nbsp;` for readability. */
export function escText(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/ /g, '&nbsp;')
}

export function escAttr(s: string): string {
  return escText(s).replace(/"/g, '&quot;')
}
