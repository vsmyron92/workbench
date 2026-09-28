import { describe, expect, it } from 'vitest'
import { getSchema } from '@tiptap/core'
import { docToStorage, storageToDoc, type PMNode } from './convert'
import { editorExtensions } from './schema'
import { safeInlineStyle } from './style'

const schema = getSchema(editorExtensions())

const OVERLAY =
  'position:fixed;top:0;left:0;width:100vw;height:100vh;z-index:2147483647;background:#b00;color:#fff;font-size:40px'

describe('safeInlineStyle', () => {
  it('keeps text colour and emphasis', () => {
    expect(safeInlineStyle('color: rgb(151,160,175);')).toBe('color: rgb(151,160,175)')
    expect(safeInlineStyle('background-color:#FFF0B3;text-decoration: underline; font-weight: bold')).toBe(
      'background-color: #FFF0B3; text-decoration: underline; font-weight: bold',
    )
    expect(safeInlineStyle('font-style: italic; letter-spacing: 0.0px;')).toBe('font-style: italic')
  })

  it('drops layout, layering and sizing', () => {
    expect(safeInlineStyle(OVERLAY)).toBe('color: #fff')
    for (const s of ['position: absolute', 'inset: 0', 'z-index: 9', 'display: block', 'width: 100vw', 'transform: scale(40)', 'margin-top: -900px']) {
      expect(safeInlineStyle(s)).toBeNull()
    }
  })

  it('drops anything that could load a resource or escape the value', () => {
    for (const s of [
      'background-color: url(https://evil.example/t.png)',
      'color: red; background-color: image-set("x.png" 1x)',
      'color: var(--x)',
      'color: u\\72l(https://evil.example)',
      'color: red !important',
      'color: /* c */ red',
      'color: expression(alert(1))',
    ]) {
      expect(safeInlineStyle(s)?.includes('evil') ?? false).toBe(false)
      expect(safeInlineStyle(s) ?? '').not.toMatch(/url|image-set|var|expression|\\|!|\/\*/i)
    }
    expect(safeInlineStyle('color: red; background-color: url(x)')).toBe('color: red')
  })

  it('handles empty input', () => {
    expect(safeInlineStyle(null)).toBeNull()
    expect(safeInlineStyle('')).toBeNull()
    expect(safeInlineStyle(';;:')).toBeNull()
  })
})

describe('CfSpan in the editor', () => {
  const storage = `<p>Normal text <span style="${OVERLAY}">WORKBENCH SESSION EXPIRED</span></p>`

  /** What the editor puts in the DOM for the span mark (the DOMOutputSpec's attributes). */
  function renderedSpanAttrs(doc: PMNode): Record<string, string> {
    const node = schema.nodeFromJSON(doc)
    let attrs: Record<string, string> | null = null
    node.descendants((n) => {
      const m = n.marks.find((x) => x.type.name === 'cfSpan')
      if (m && !attrs) attrs = (m.type.spec.toDOM!(m, true) as unknown as [string, Record<string, string>])[1]
    })
    return attrs!
  }

  it('renders only the safe part of a stored style', () => {
    const attrs = renderedSpanAttrs(storageToDoc(storage))
    expect(attrs.style).toBe('color: #fff')
    expect(JSON.stringify(attrs)).not.toMatch(/fixed|z-index|100vw|inset/)
  })

  it('renders no style attribute when nothing is safe', () => {
    const attrs = renderedSpanAttrs(storageToDoc('<p><span style="position:fixed;inset:0">x</span></p>'))
    expect(attrs.style).toBeUndefined()
  })

  it('still writes the original style back byte-for-byte', () => {
    const doc = schema.nodeFromJSON(storageToDoc(storage)).toJSON() as PMNode
    expect(docToStorage(doc)).toBe(storage)
  })
})
