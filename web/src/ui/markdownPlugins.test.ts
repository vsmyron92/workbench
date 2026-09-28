import { describe, expect, it } from 'vitest'
import { anchorIds, codeLanguage, hastText, rehypeAlerts, splitFrontmatter, type HastNode } from './markdownPlugins'

describe('splitFrontmatter', () => {
  it('splits a leading YAML block', () => {
    const r = splitFrontmatter('---\ntitle: Roadmap\ntags: [a]\n---\n# Hello\n')
    expect(r.frontmatter).toBe('title: Roadmap\ntags: [a]')
    expect(r.body).toBe('# Hello\n')
  })
  it('handles CRLF, a BOM and the ... terminator', () => {
    const r = splitFrontmatter('﻿---\r\na: 1\r\n...\r\nbody')
    expect(r.frontmatter).toBe('a: 1')
    expect(r.body).toBe('body')
  })
  it('leaves documents without front matter alone', () => {
    expect(splitFrontmatter('# Title\n\n---\nnot front matter').frontmatter).toBeNull()
    expect(splitFrontmatter('text\n---\nx\n---\n').frontmatter).toBeNull()
  })
})

const p = (text: string, ...rest: HastNode[]): HastNode => ({ type: 'element', tagName: 'p', children: [{ type: 'text', value: text }, ...rest] })
const bq = (...children: HastNode[]): HastNode => ({ type: 'element', tagName: 'blockquote', children })

describe('rehypeAlerts', () => {
  it('turns [!NOTE] blockquotes into callouts and strips the marker', () => {
    const quote = bq({ type: 'text', value: '\n' }, p('[!NOTE]\nRead this first.'))
    const tree: HastNode = { type: 'root', children: [quote] }
    rehypeAlerts()(tree)
    expect(quote.properties?.className).toEqual(['wb-alert', 'wb-alert-note'])
    expect(hastText(quote).trim()).toBe('Read this first.')
  })
  it('drops the line break after a marker on its own line', () => {
    const quote = bq(p('[!warning]', { type: 'element', tagName: 'br' }, { type: 'text', value: 'Careful.' }))
    rehypeAlerts()({ type: 'root', children: [quote] })
    expect(quote.properties?.className).toEqual(['wb-alert', 'wb-alert-warning'])
    expect(quote.children?.[0].children?.map((c) => c.tagName ?? c.value)).toEqual(['', 'Careful.'])
  })
  it('ignores ordinary blockquotes and unknown kinds', () => {
    const a = bq(p('Just a quote'))
    const b = bq(p('[!DANGER] not a GitHub kind'))
    rehypeAlerts()({ type: 'root', children: [a, b] })
    expect(a.properties).toBeUndefined()
    expect(b.properties).toBeUndefined()
  })
})

describe('anchorIds', () => {
  it('tries the anchor as written, as a heading slug and as a sanitized document id', () => {
    expect(anchorIds('#fn-1')).toEqual(['fn-1', 'md-fn-1', 'user-content-fn-1'])
    expect(anchorIds('Setup')).toEqual(['Setup', 'md-Setup', 'md-setup', 'user-content-Setup'])
    expect(anchorIds('caf%C3%A9')).toContain('md-café')
    expect(anchorIds('%E0%A4%A')).toContain('md-%e0%a4%a')
    expect(anchorIds('#')).toEqual([])
  })
})

describe('codeLanguage', () => {
  it('reads the language class', () => {
    expect(codeLanguage(['hljs', 'language-rust'])).toBe('rust')
    expect(codeLanguage('language-mermaid')).toBe('mermaid')
    expect(codeLanguage(['hljs'])).toBeNull()
    expect(codeLanguage(undefined)).toBeNull()
  })
})
