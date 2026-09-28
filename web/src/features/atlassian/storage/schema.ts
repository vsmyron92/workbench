// TipTap extensions for editing Confluence storage format. `convert.ts` produces and
// consumes documents in this schema; `editorExtensions()` is what the editor (and the
// schema tests) use.

import { Extension, Mark, Node, markInputRule, mergeAttributes, type AnyExtension } from '@tiptap/core'
import StarterKit from '@tiptap/starter-kit'
import { TableKit } from '@tiptap/extension-table'
import { safeInlineStyle } from './style'

/** An attribute that lives only in the document JSON (never rendered to or parsed from HTML). */
const hidden = (keepOnSplit = false) => ({ default: null, rendered: false, keepOnSplit })

/** Extra storage attributes (`xattrs`), synthesized-paragraph flags and code/table details. */
export const CfAttributes = Extension.create({
  name: 'cfAttributes',
  addGlobalAttributes() {
    return [
      {
        types: ['paragraph', 'heading', 'bulletList', 'orderedList', 'listItem', 'blockquote', 'table', 'tableRow', 'tableCell', 'tableHeader'],
        attributes: { xattrs: hidden() },
      },
      { types: ['paragraph'], attributes: { synth: hidden() } },
      { types: ['codeBlock', 'table'], attributes: { cf: hidden() } },
      { types: ['tableRow'], attributes: { section: hidden(true) } },
      { types: ['link'], attributes: { xattrs: hidden(true) } },
      // Which tag a mark came from (<b> vs <strong>, <del> vs <s>) so it is written back the same.
      { types: ['bold', 'italic', 'strike'], attributes: { tag: hidden(true) } },
    ]
  },
})

const atomAttrs = () => ({
  xml: { default: '', rendered: false },
  kind: { default: '', rendered: false },
  label: { default: '', rendered: false },
  preview: { default: null, rendered: false },
})

const atomDom = (tag: 'div' | 'span', attrs: Record<string, unknown>) => {
  const kind = String(attrs.kind ?? '')
  const label = String(attrs.label ?? '')
  const preview = attrs.preview ? String(attrs.preview) : null
  const base = {
    class: `cf-atom cf-atom-${tag === 'div' ? 'block' : 'inline'} cf-kind-${kind.replace(/[^a-z0-9-]/gi, '-')}`,
    'data-cf-xml': String(attrs.xml ?? ''),
    'data-cf-kind': kind,
    'data-cf-label': label,
    'data-cf-preview': preview ?? '',
    'data-status-colour': kind === 'status' && preview ? preview : undefined,
    contenteditable: 'false',
    title: kind === 'status' ? 'Status' : `${kind || 'macro'} (kept as is)`,
  }
  if (kind === 'image' && preview) {
    return [tag, base, ['img', { src: preview, alt: label, loading: 'lazy' }]] as const
  }
  if (tag === 'div') return [tag, base, ['span', { class: 'cf-atom-kind' }, kind || 'macro'], ['span', { class: 'cf-atom-label' }, label]] as const
  return [tag, base, label || kind] as const
}

const parseAtom = (el: HTMLElement) => ({
  xml: el.getAttribute('data-cf-xml') ?? '',
  kind: el.getAttribute('data-cf-kind') ?? '',
  label: el.getAttribute('data-cf-label') ?? '',
  preview: el.getAttribute('data-cf-preview') || null,
})

/** A block element the editor keeps verbatim (macros, task lists, unknown markup). */
export const CfBlock = Node.create({
  name: 'cfBlock',
  group: 'block',
  atom: true,
  selectable: true,
  draggable: true,
  addAttributes: atomAttrs,
  parseHTML: () => [{ tag: 'div[data-cf-xml]', getAttrs: (el) => parseAtom(el as HTMLElement) }],
  renderHTML: ({ node }) => atomDom('div', node.attrs) as never,
})

/** An inline element kept verbatim (page links, mentions, images, status lozenges). */
export const CfInline = Node.create({
  name: 'cfInline',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: true,
  addAttributes: atomAttrs,
  parseHTML: () => [{ tag: 'span[data-cf-xml]', getAttrs: (el) => parseAtom(el as HTMLElement) }],
  renderHTML: ({ node }) => atomDom('span', node.attrs) as never,
})

/** A macro with an editable rich-text body (info, note, warning, panel, expand…). */
export const CfRich = Node.create({
  name: 'cfRich',
  group: 'block',
  content: 'block+',
  defining: true,
  isolating: true,
  addAttributes: () => ({
    tpl: { default: '', rendered: false },
    name: { default: 'panel', rendered: false },
    title: { default: '', rendered: false },
  }),
  parseHTML: () => [
    {
      tag: 'div[data-cf-rich]',
      contentElement: '.cf-rich-body',
      getAttrs: (el) => {
        const e = el as HTMLElement
        return { tpl: e.getAttribute('data-cf-tpl') ?? '', name: e.getAttribute('data-cf-rich') ?? 'panel', title: e.getAttribute('data-cf-title') ?? '' }
      },
    },
  ],
  renderHTML: ({ node }) => {
    const name = String(node.attrs.name)
    const head = node.attrs.title ? `${name}: ${node.attrs.title}` : name
    return [
      'div',
      { class: `cf-rich cf-rich-${name.replace(/[^a-z0-9-]/gi, '-')}`, 'data-cf-rich': name, 'data-cf-tpl': node.attrs.tpl, 'data-cf-title': node.attrs.title },
      ['div', { class: 'cf-rich-head', contenteditable: 'false' }, head],
      ['div', { class: 'cf-rich-body' }, 0],
    ]
  },
})

const xattrsAttr = () => ({ xattrs: { default: null, rendered: false } })

/** Page layouts: `ac:layout` > `ac:layout-section` > `ac:layout-cell`. */
export const CfLayout = Node.create({
  name: 'cfLayout',
  group: 'block',
  content: 'cfSection+',
  isolating: true,
  addAttributes: xattrsAttr,
  parseHTML: () => [{ tag: 'div[data-cf-layout]' }],
  renderHTML: () => ['div', { class: 'cf-layout', 'data-cf-layout': '' }, 0],
})

export const CfSection = Node.create({
  name: 'cfSection',
  content: 'cfCell+',
  isolating: true,
  addAttributes: xattrsAttr,
  parseHTML: () => [{ tag: 'div[data-cf-section]' }],
  renderHTML: ({ node }) => {
    const type = (node.attrs.xattrs as [string, string][] | null)?.find(([k]) => k === 'ac:type')?.[1] ?? 'single'
    return ['div', { class: `cf-section cf-section-${type.replace(/[^a-z0-9_-]/gi, '')}`, 'data-cf-section': '' }, 0]
  },
})

export const CfCell = Node.create({
  name: 'cfCell',
  content: 'block+',
  isolating: true,
  addAttributes: xattrsAttr,
  parseHTML: () => [{ tag: 'div[data-cf-cell]' }],
  renderHTML: () => ['div', { class: 'cf-cell', 'data-cf-cell': '' }, 0],
})

/** An inline-comment marker: the commented text stays editable, the marker survives. */
export const CfComment = Mark.create({
  name: 'cfComment',
  inclusive: false,
  excludes: '',
  addAttributes: () => ({
    ref: { default: null, parseHTML: (el) => el.getAttribute('data-cf-comment'), renderHTML: (a) => ({ 'data-cf-comment': a.ref }) },
  }),
  parseHTML: () => [{ tag: 'span[data-cf-comment]' }],
  renderHTML: ({ HTMLAttributes }) => ['span', mergeAttributes({ class: 'cf-comment-mark', title: 'Inline comment' }, HTMLAttributes), 0],
})

/**
 * A `<span>` with attributes (colours, classes) kept as-is in `xattrs`. Only a safe
 * subset of its style is shown in the editor (see ./style.ts): the page author must
 * not be able to lay anything over Workbench's own UI.
 */
export const CfSpan = Mark.create({
  name: 'cfSpan',
  excludes: '',
  addAttributes: () => ({ xattrs: { default: null, rendered: false } }),
  parseHTML: () => [],
  renderHTML: ({ mark }) => {
    const style = safeInlineStyle((mark.attrs.xattrs as [string, string][] | null)?.find(([k]) => k === 'style')?.[1])
    return ['span', style ? { style } : {}, 0]
  },
})

/**
 * Inline code. StarterKit's version excludes every other mark, which would reject
 * `<strong><code>` (common in real pages), so this one combines with the others.
 */
export const CfCode = Mark.create({
  name: 'code',
  excludes: '',
  code: true,
  exitable: true,
  parseHTML: () => [{ tag: 'code' }],
  renderHTML: () => ['code', 0],
  addKeyboardShortcuts() {
    return { 'Mod-e': () => this.editor.commands.toggleMark(this.name) }
  },
  addInputRules() {
    return [markInputRule({ find: /(?:^|[^`])(`([^`]+)`)$/, type: this.type })]
  },
})

export const CfSub = Mark.create({
  name: 'cfSub',
  excludes: 'cfSup',
  parseHTML: () => [{ tag: 'sub' }],
  renderHTML: () => ['sub', 0],
})

export const CfSup = Mark.create({
  name: 'cfSup',
  excludes: 'cfSub',
  parseHTML: () => [{ tag: 'sup' }],
  renderHTML: () => ['sup', 0],
})

/** Everything the Confluence editor needs. */
export function editorExtensions(): AnyExtension[] {
  return [
    StarterKit.configure({
      // A trailing empty paragraph would be written back into the page.
      trailingNode: false,
      code: false,
      link: { openOnClick: false, autolink: true, HTMLAttributes: { target: null, rel: null, class: null } },
    }),
    TableKit.configure({ table: { resizable: false } }),
    CfAttributes,
    CfBlock,
    CfInline,
    CfRich,
    CfLayout,
    CfSection,
    CfCell,
    CfComment,
    CfSpan,
    CfCode,
    CfSub,
    CfSup,
  ]
}
