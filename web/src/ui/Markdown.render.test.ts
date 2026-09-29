// The whole Markdown pipeline, rendered on the server: what untrusted repository
// markdown can and cannot put on the page.

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { Markdown } from './Markdown'

const render = (text: string) => renderToStaticMarkup(createElement(Markdown, { text }))

describe('Markdown', () => {
  it('drops scripts, event handlers, styles and javascript: URLs from raw HTML', () => {
    const html = render(
      [
        '<script>alert(1)</script>',
        '<img src="x.png" onerror="alert(2)" style="position:fixed">',
        '<a href="javascript:alert(3)">x</a>',
        '<iframe src="https://example.com"></iframe>',
        '[y](javascript:alert(4))',
      ].join('\n\n'),
    )
    expect(html).not.toMatch(/<script|onerror|alert\(|style=|<iframe|javascript:/i)
    expect(html).toContain('src="x.png"')
  })

  it('keeps the HTML GitHub allows', () => {
    const html = render('<details><summary>More</summary>\n\nHidden *text*\n\n</details>\n\nPress <kbd>Ctrl</kbd>+<kbd>S</kbd>, H<sub>2</sub>O')
    expect(html).toContain('<details><summary>More</summary>')
    expect(html).toContain('<em>text</em>')
    expect(html).toContain('<kbd>Ctrl</kbd>')
    expect(html).toContain('<sub>2</sub>')
  })

  it('prefixes ids from the document so they cannot clobber the page', () => {
    const html = render('<a name="top"></a>\n\n<p id="app">x</p>')
    expect(html).toContain('name="user-content-top"')
    expect(html).toContain('id="user-content-app"')
  })

  it('gives headings GitHub slugs with an anchor link', () => {
    const html = render('# Getting Started\n\n## Getting Started')
    expect(html).toContain('id="md-getting-started"')
    expect(html).toContain('id="md-getting-started-1"')
    expect(html).toContain('href="#getting-started"')
  })

  it('renders alerts, footnotes, task lists and front matter', () => {
    const html = render('---\ntitle: Plan\n---\n> [!WARNING]\n> Mind the gap.\n\n- [x] done\n\nSee[^1].\n\n[^1]: The note.')
    expect(html).toContain('wb-alert wb-alert-warning')
    expect(html).toMatch(/<div class="wb-alert-title"><svg[^>]*>.*<\/svg>Warning<\/div>/)
    expect(html).not.toContain('[!WARNING]')
    expect(html).toContain('type="checkbox"')
    expect(html).toContain('href="#fn-1"')
    expect(html).toContain('id="user-content-fn-1"')
    expect(html).toContain('<summary>Front matter</summary>')
    expect(html).toContain('title: Plan')
  })

  it('labels code blocks and highlights known languages', () => {
    const html = render('```rust\nfn main() {}\n```')
    expect(html).toContain('<span>rust</span>')
    expect(html).toContain('hljs-keyword')
    expect(html).toContain('aria-label="Copy code"')
  })

  it('highlights C, C++, Verilog and VHDL', () => {
    for (const fence of ['c\nint main(void) { return 0; }', 'c++\ntemplate <class T> struct S {};', 'systemverilog\nmodule top; endmodule', 'vhdl\nENTITY top IS END ENTITY;']) {
      expect(render('```' + fence + '\n```'), fence).toContain('hljs-keyword')
    }
  })
})
