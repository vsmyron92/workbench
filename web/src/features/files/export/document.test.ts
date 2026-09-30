import { describe, expect, it } from 'vitest'
import { setHealth } from '@/api/health'
import { documentHtml, embeddableImagePath, escapeHtml, exportFileName, exportPath, fetchEmbeddableImage, IMAGE_CAP, ImageSkip, resolveVars, tocHtml, TOTAL_IMAGE_CAP, varNames } from './document'

describe('resolveVars', () => {
  const tokens: Record<string, string> = { '--bg': '#fff', '--fg': ' #111 ', '--accent': 'var(--blue)', '--blue': '#36f' }
  const lookup = (n: string) => tokens[n]

  it('replaces known tokens, recursively, and keeps unknown ones', () => {
    expect(resolveVars('body{background:var(--bg);color:var(--fg)}', lookup)).toBe('body{background:#fff;color:#111}')
    expect(resolveVars('a{color:var(--accent)}', lookup)).toBe('a{color:#36f}')
    // A document's own custom property stays a variable.
    expect(resolveVars('.x{--alert:var(--blue)} .y{color:var(--alert)}', lookup)).toBe('.x{--alert:#36f} .y{color:var(--alert)}')
  })

  it('resolves inside fallbacks and nested functions', () => {
    expect(resolveVars('x{c:var(--nope, var(--bg))}', lookup)).toBe('x{c:var(--nope, #fff)}')
    expect(resolveVars('x{b:color-mix(in srgb, var(--blue) 10%, transparent)}', lookup)).toBe('x{b:color-mix(in srgb, #36f 10%, transparent)}')
    expect(resolveVars('x{c:var(--bg, rgba(0, 0, 0, 0.5))}', lookup)).toBe('x{c:#fff}')
  })

  it('survives broken input and loops', () => {
    expect(resolveVars('x{c:var(--bg', lookup)).toBe('x{c:var(--bg')
    const loop = (n: string) => (n === '--a' ? 'var(--a)' : undefined)
    expect(() => resolveVars('x{c:var(--a)}', loop)).not.toThrow()
  })

  it('lists the variables a sheet uses', () => {
    expect(varNames('a{c:var(--fg);b:var( --bg , red);d:var(--fg)}')).toEqual(['--fg', '--bg'])
  })
})

describe('names and escaping', () => {
  it('derives the export name', () => {
    expect(exportFileName('README.md')).toBe('README.html')
    expect(exportFileName('docs/guide.markdown')).toBe('guide.html')
    expect(exportFileName('.notes')).toBe('.notes.html')
    expect(exportPath('docs/guide.md')).toBe('docs/guide.html')
    expect(exportPath('README.md')).toBe('README.html')
  })

  it('escapes html', () => {
    expect(escapeHtml(`<a href="x">'&'</a>`)).toBe('&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;')
  })
})

describe('tocHtml', () => {
  it('lists headings, escaped, only when there are at least two', () => {
    expect(tocHtml([{ id: 'md-a', text: 'A', level: 1 }])).toBe('')
    const html = tocHtml([
      { id: 'md-intro', text: 'Intro <1>', level: 2 },
      { id: 'md-use', text: 'Use', level: 3 },
    ])
    expect(html).toContain('<a class="wb-md-toc-item l2" href="#md-intro">Intro &lt;1&gt;</a>')
    expect(html).toContain('class="wb-md-toc-item l3"')
  })
})

describe('documentHtml', () => {
  it('is a standalone page that runs nothing', () => {
    const html = documentHtml({ title: 'A <b>', theme: 'light', css: 'body{}', body: '<p>x</p>', toc: '', source: 'README.md', exportedAt: new Date(2026, 8, 27) })
    expect(html.startsWith('<!doctype html>')).toBe(true)
    expect(html).toContain('<title>A &lt;b&gt;</title>')
    expect(html).toContain('data-theme="light"')
    expect(html).toContain("default-src 'none'")
    // Relative images next to the exported file load when it is opened from disk.
    const csp = /Content-Security-Policy" content="([^"]*)"/.exec(html)![1]
    const img = csp.split(';').map((d) => d.trim()).find((d) => d.startsWith('img-src '))!
    expect(img.split(' ')).toEqual(expect.arrayContaining(["'self'", 'file:', 'data:']))
    expect(csp).not.toMatch(/script-src|unsafe-eval/)
    expect(html).not.toMatch(/<script/i)
    expect(html).toContain('<article class="wb-prose">\n<p>x</p>\n</article>')
    expect(html).toContain('Exported from README.md')
  })
})

describe('embedding images', () => {
  const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
  const respond = (body: BodyInit, type: string, status = 200) => async () => new Response(body, { status, headers: { 'content-type': type } })
  const reason = async (p: Promise<unknown>) => {
    try {
      await p
      return 'embedded'
    } catch (e) {
      expect(e).toBeInstanceOf(ImageSkip)
      return (e as Error).message
    }
  }

  it('only fetches image files outside .git', () => {
    expect(embeddableImagePath('docs/logo.png')).toBe(true)
    expect(embeddableImagePath('/home/u/notes/shot.JPEG')).toBe(true)
    expect(embeddableImagePath('art/diagram.svg')).toBe(true)
    expect(embeddableImagePath('.git/config')).toBe(false)
    expect(embeddableImagePath('.git/logo.png')).toBe(false)
    expect(embeddableImagePath('sub/.git/x.gif')).toBe(false)
    expect(embeddableImagePath('target/local.yml')).toBe(false)
    expect(embeddableImagePath('src/lib.rs')).toBe(false)
    expect(embeddableImagePath('logo.png.txt')).toBe(false)
  })

  it('reads a document at a Windows drive path on a Windows server', () => {
    setHealth({ ok: true, service: 'workbench', version: '0', startedAt: 1, os: 'windows' })
    try {
      expect(embeddableImagePath('C:\\notes\\shot.png')).toBe(true)
      expect(embeddableImagePath('docs/logo.png')).toBe(true)
      // `\` separates and case does not matter there: all of these are in `.git`.
      expect(embeddableImagePath('C:\\p\\.git\\logo.png')).toBe(false)
      expect(embeddableImagePath('C:\\p\\.GIT\\logo.png')).toBe(false)
      expect(embeddableImagePath('C:/p/.Git/logo.png')).toBe(false)
      expect(embeddableImagePath('sub/.GIT/x.gif')).toBe(false)
      expect(exportFileName('C:\\notes\\plan.md')).toBe('plan.html')
      expect(exportPath('C:\\notes\\plan.md')).toBe('C:\\notes\\plan.html')
      expect(exportPath('docs/guide.md')).toBe('docs/guide.html')
    } finally {
      setHealth(null)
    }
  })

  it('never fetches or embeds ../.git/config, whatever the server says', async () => {
    let fetched = 0
    const fetcher = async () => {
      fetched++
      return new Response('[remote "origin"]\n\turl = https://oauth2:TOKEN@example.invalid/x.git\n', { headers: { 'content-type': 'image/png' } })
    }
    expect(await reason(fetchEmbeddableImage('.git/config', '/raw?path=.git/config', 0, fetcher))).toBe('not an image, linked instead')
    expect(fetched).toBe(0)
  })

  it('embeds only what is served as an image, within the caps', async () => {
    const blob = await fetchEmbeddableImage('docs/logo.png', 'u', 0, respond(png, 'image/png'))
    expect(blob.size).toBe(png.length)
    // A file named like an image that is not one.
    expect(await reason(fetchEmbeddableImage('docs/fake.png', 'u', 0, respond('db_password: hunter2', 'text/plain; charset=utf-8')))).toBe('not an image, linked instead')
    expect(await reason(fetchEmbeddableImage('docs/x.png', 'u', 0, respond('', 'application/octet-stream')))).toBe('not an image, linked instead')
    expect(await reason(fetchEmbeddableImage('.env.png', 'u', 0, respond('', 'text/plain', 403)))).toBe('marked sensitive, not embedded')
    expect(await reason(fetchEmbeddableImage('gone.png', 'u', 0, respond('', 'text/plain', 404)))).toMatch(/HTTP 404/)
    expect(await reason(fetchEmbeddableImage('big.png', 'u', 0, respond(new Uint8Array(IMAGE_CAP + 1), 'image/png')))).toMatch(/^over 5 MB/)
    expect(await reason(fetchEmbeddableImage('one.png', 'u', TOTAL_IMAGE_CAP - 4, respond(png, 'image/png')))).toMatch(/in all/)
  })
})
