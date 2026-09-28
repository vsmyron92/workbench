import { describe, expect, it } from 'vitest'
import designStorage from '../../../../../server/src/atlassian/testdata/design_storage.xml?raw'
import { getSchema } from '@tiptap/core'
import {
  attachmentXml,
  docToStorage,
  formatForDiff,
  inlineAtom,
  markerRefs,
  mentionXml,
  pageLinkXml,
  storageToDoc,
  storageToText,
  type PMNode,
} from './convert'
import { editorExtensions } from './schema'
import { parseXml, XmlError } from './xml'

const schema = getSchema(editorExtensions())

/** Load through ProseMirror (validating the schema) and back to JSON, like the editor does. */
function throughEditor(doc: PMNode): PMNode {
  const node = schema.nodeFromJSON(doc)
  node.check()
  return node.toJSON() as PMNode
}

function roundTrip(storage: string, opts = {}) {
  const doc = storageToDoc(storage, opts)
  const direct = docToStorage(doc)
  const viaEditor = docToStorage(throughEditor(doc))
  return { doc, direct, viaEditor }
}

/** Assert equality, reporting where two long strings first differ. */
function expectSame(actual: string, expected: string) {
  if (actual === expected) return
  let i = 0
  while (i < actual.length && actual[i] === expected[i]) i++
  expect(actual.slice(Math.max(0, i - 60), i + 80)).toBe(expected.slice(Math.max(0, i - 60), i + 80))
}

const CANONICAL = [
  '<p>Plain <strong>bold</strong>, <em>em</em>, <u>u</u>, <s>gone</s>, <code>code</code>, x<sub>2</sub> y<sup>3</sup>.</p>',
  '<h1>Title &amp; more</h1><h2>Two</h2><p />',
  '<ul><li><p>one</p></li><li><p>two</p><ol start="3"><li><p>nested</p></li></ol></li></ul>',
  '<ul><li>bare text</li><li><ul><li><p>nested first</p></li></ul></li></ul>',
  '<table data-layout="default" ac:local-id="t1"><colgroup><col style="width: 170.0px;" /></colgroup><tbody><tr><th><p>A</p></th><th colspan="2"><p>B</p></th></tr><tr><td><p>1</p></td><td>2</td><td class="highlight-grey" data-highlight-colour="grey"><p>3</p><p>4</p></td></tr></tbody></table>',
  '<table><thead><tr><th>h</th></tr></thead><tbody><tr><td>b</td></tr></tbody></table>',
  '<ac:structured-macro ac:name="code" ac:schema-version="1" ac:macro-id="m1"><ac:parameter ac:name="title">T</ac:parameter><ac:parameter ac:name="language">rust</ac:parameter><ac:plain-text-body><![CDATA[if a < b && c { run(); }\n]]></ac:plain-text-body></ac:structured-macro>',
  '<ac:structured-macro ac:name="info" ac:schema-version="1"><ac:parameter ac:name="title">Heads up</ac:parameter><ac:rich-text-body><p>Info <strong>body</strong></p></ac:rich-text-body></ac:structured-macro>',
  '<p>See <ac:link><ri:page ri:content-title="Runbook" /></ac:link> &amp; ping <ac:link><ri:user ri:account-id="abc" /></ac:link> <ac:structured-macro ac:name="status" ac:schema-version="1"><ac:parameter ac:name="colour">Green</ac:parameter><ac:parameter ac:name="title">Done</ac:parameter></ac:structured-macro></p>',
  '<p>Before <ac:inline-comment-marker ac:ref="r1">commented <strong>bold</strong> text</ac:inline-comment-marker> after</p>',
  '<p><a href="https://x.dev/?a=1&amp;b=2" title="t">link <em>text</em></a> and <span style="color: rgb(151,160,175);">grey</span></p>',
  '<pre>raw &lt;pre&gt;\n  text</pre><hr /><blockquote><p>quoted</p></blockquote>',
  '<p>line<br />break&nbsp;nbsp</p>',
  '<ac:task-list><ac:task><ac:task-id>1</ac:task-id><ac:task-status>complete</ac:task-status><ac:task-body>done</ac:task-body></ac:task></ac:task-list>',
  '<ac:layout><ac:layout-section ac:type="two_equal" ac:breakout-mode="default"><ac:layout-cell><p>left</p></ac:layout-cell><ac:layout-cell><h3>right</h3></ac:layout-cell></ac:layout-section></ac:layout>',
  '<p><ac:image ac:align="center"><ri:attachment ri:filename="x.png" /></ac:image></p><ac:image><ri:url ri:value="https://example.com/a.png" /></ac:image>',
  '<div class="legacy">kept <b>verbatim</b></div><p><strong style="color: red;">styled</strong></p>',
  '<p><b>bee</b> <i>eye</i> <del>dee</del></p>',
]

describe('storage ↔ editor document', () => {
  it.each(CANONICAL)('round-trips %s', (storage) => {
    const { direct, viaEditor } = roundTrip(storage)
    expect(direct).toBe(storage)
    expect(viaEditor).toBe(storage)
  })

  it('maps structure to editable nodes', () => {
    const doc = storageToDoc(CANONICAL[6] + CANONICAL[7] + CANONICAL[9])
    const types = doc.content!.map((n) => n.type)
    expect(types).toEqual(['codeBlock', 'cfRich', 'paragraph'])
    expect(doc.content![0].attrs!.language).toBe('rust')
    expect(doc.content![0].content![0].text).toBe('if a < b && c { run(); }\n')
    const marked = doc.content![2].content!.find((n) => n.marks?.some((m) => m.type === 'cfComment'))
    expect(marked?.text).toBe('commented ')
  })

  it('rebuilds an edited code macro, keeping its other parameters', () => {
    const doc = storageToDoc(CANONICAL[6])
    const code = doc.content![0]
    code.content = [{ type: 'text', text: 'let x = "$&]]>";' }]
    code.attrs = { ...code.attrs, language: 'typescript' }
    expect(docToStorage(doc)).toBe(
      '<ac:structured-macro ac:name="code" ac:schema-version="1" ac:macro-id="m1"><ac:parameter ac:name="title">T</ac:parameter><ac:parameter ac:name="language">typescript</ac:parameter><ac:plain-text-body><![CDATA[let x = "$&]]]]><![CDATA[>";]]></ac:plain-text-body></ac:structured-macro>',
    )
  })

  it('writes new content in Confluence style', () => {
    const doc: PMNode = {
      type: 'doc',
      content: [
        { type: 'paragraph', content: [{ type: 'text', text: 'a < b', marks: [{ type: 'bold' }] }] },
        { type: 'codeBlock', attrs: { language: 'sh' }, content: [{ type: 'text', text: 'ls' }] },
        { type: 'paragraph' },
        {
          type: 'table',
          content: [{ type: 'tableRow', content: [{ type: 'tableCell', attrs: { colspan: 1, rowspan: 1 }, content: [{ type: 'paragraph' }] }] }],
        },
      ],
    }
    expect(docToStorage(throughEditor(doc))).toBe(
      '<p><strong>a &lt; b</strong></p><ac:structured-macro ac:name="code" ac:schema-version="1"><ac:parameter ac:name="language">sh</ac:parameter><ac:plain-text-body><![CDATA[ls]]></ac:plain-text-body></ac:structured-macro><p /><table><tbody><tr><td><p /></td></tr></tbody></table>',
    )
  })

  it('keeps inline-comment markers when the commented text is edited', () => {
    const doc = storageToDoc(CANONICAL[9])
    const t = doc.content![0].content!.find((n) => n.text === 'commented ')!
    t.text = 'reworded '
    const out = docToStorage(doc)
    expect(markerRefs(out)).toEqual(['r1'])
    expect(out).toContain('<ac:inline-comment-marker ac:ref="r1">reworded <strong>bold</strong> text</ac:inline-comment-marker>')
  })

  it('previews attachment images of the page', () => {
    const doc = storageToDoc(CANONICAL[15], { pageId: '42' })
    const img = doc.content![0].content![0]
    expect(img.type).toBe('cfInline')
    expect(img.attrs!.preview).toBe('/api/confluence/attachments/42/by-name/x.png')
  })

  it('handles an empty page', () => {
    const { doc, viaEditor } = roundTrip('')
    expect(doc.content).toHaveLength(1)
    expect(viaEditor).toBe('')
  })

  it('normalizes only whitespace that contains newlines', () => {
    expect(docToStorage(storageToDoc('<p>a\n   b  c</p>'))).toBe('<p>a b  c</p>')
  })

  it('round-trips the design template page', () => {
    const storage = designStorage
    const { doc, viaEditor } = roundTrip(storage, { pageId: '229492' })
    expect(doc.content![0].type).toBe('cfLayout')
    // Entities are decoded (&rsquo; → ’) and whitespace between blocks dropped; otherwise byte-identical.
    expectSame(viaEditor, storage.trimEnd().replace(/&rsquo;/g, '’').replace(/&acirc;/g, 'â'))
  })

  it('refuses malformed storage with a position', () => {
    expect(() => storageToDoc('<p>one</p>\n<p>two <b></p>')).toThrow(XmlError)
    try {
      parseXml('<p>a</p>\n<p>&bogus;</p>')
    } catch (e) {
      expect((e as XmlError).line).toBe(2)
    }
  })
})

describe('diff helpers', () => {
  it('breaks storage into lines at block boundaries', () => {
    expect(formatForDiff('<h1>T</h1><p>a</p><ul><li><p>x</p></li></ul>')).toBe('<h1>T</h1>\n<p>a</p>\n<ul>\n<li><p>x</p>\n</li>\n</ul>\n')
  })

  it('renders plain text', () => {
    expect(storageToText('<h2>T</h2><p>a <strong>b</strong></p><ul><li><p>x</p></li></ul><table><tbody><tr><td>1</td><td>2</td></tr></tbody></table>')).toBe(
      '## T\na b\n• x\n| 1\n| 2',
    )
  })
})

// The owner's real pages (not committed): WORKBENCH_ATLASSIAN_SAMPLES=<dir with st_*.xml>.
// Node APIs are reached untyped: the app's tsconfig has browser types only.
interface NodeFs {
  readFileSync(path: string, encoding: 'utf8'): string
  readdirSync(path: string): string[]
  existsSync(path: string): boolean
}
const env = (globalThis as unknown as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {}
const samples = env.WORKBENCH_ATLASSIAN_SAMPLES
const fs: NodeFs | null = samples ? ((await import(/* @vite-ignore */ `node:${'fs'}`)) as NodeFs) : null
const sampleFiles: string[] =
  samples && fs?.existsSync(samples) ? fs.readdirSync(samples).filter((f) => /^st_\d+\.xml$/.test(f) || f === 'design_storage.xml') : []
describe.skipIf(!sampleFiles.length)('research samples', () => {
  it.each(sampleFiles)('%s survives the editor', (f: string) => {
    const storage = fs!.readFileSync(`${samples}/${f}`, 'utf8')
    const { direct, viaEditor } = roundTrip(storage)
    expect(viaEditor).toBe(direct)
    // Only entity spelling and newline-whitespace may differ from the original.
    const norm = (s: string) => storageToText(s)
    expect(norm(viaEditor)).toBe(norm(storage))
  })
})

describe('mentions, page links and attachments inserted by the editor', () => {
  it('builds storage for each and round-trips it byte for byte', () => {
    const mention = mentionXml('557058:4fce-"x"')
    expect(mention).toBe('<ac:link><ri:user ri:account-id="557058:4fce-&quot;x&quot;" /></ac:link>')
    const link = pageLinkXml('Design & Review', 'DEV', 'the review ]]> page')
    expect(link).toBe(
      '<ac:link><ri:page ri:space-key="DEV" ri:content-title="Design &amp; Review" /><ac:plain-text-link-body><![CDATA[the review ]]]]><![CDATA[> page]]></ac:plain-text-link-body></ac:link>',
    )
    expect(pageLinkXml('Runbook')).toBe('<ac:link><ri:page ri:content-title="Runbook" /></ac:link>')
    const img = attachmentXml('chart 1.png', true)
    const file = attachmentXml('notes.pdf', false)
    for (const xml of [mention, link, img, file]) {
      const storage = `<p>See ${xml} here</p>`
      const { direct, viaEditor } = roundTrip(storage)
      expect(direct).toBe(storage)
      expect(viaEditor).toBe(storage)
    }
  })

  it('labels the chips with names, titles and previews', () => {
    const users = { u2: 'Ann Lee' }
    expect(inlineAtom(mentionXml('u2'), { users }).attrs).toMatchObject({ kind: 'mention', label: '@Ann Lee' })
    expect(inlineAtom(mentionXml('u9'), { users }).attrs).toMatchObject({ kind: 'mention', label: '@mention' })
    expect(inlineAtom(pageLinkXml('Runbook', null, 'steps')).attrs).toMatchObject({ kind: 'link', label: 'steps' })
    expect(inlineAtom(attachmentXml('a b.png', true), { pageId: '42' }).attrs).toMatchObject({
      kind: 'image',
      preview: '/api/confluence/attachments/42/by-name/a%20b.png',
    })
    // A mention inside a paragraph loaded with names shows the name.
    const doc = storageToDoc(`<p>${mentionXml('u2')}</p>`, { users })
    expect(doc.content?.[0].content?.[0].attrs?.label).toBe('@Ann Lee')
    // An inserted atom sits in a paragraph and serializes as its XML.
    const para: PMNode = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Hi ' }, inlineAtom(mentionXml('u2'), { users })] }],
    }
    expect(docToStorage(throughEditor(para))).toBe(`<p>Hi ${mentionXml('u2')}</p>`)
  })
})
