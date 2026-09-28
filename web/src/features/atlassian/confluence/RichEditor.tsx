// WYSIWYG editing of Confluence storage format (TipTap). Loaded lazily: TipTap is only
// downloaded when someone edits a page. Storage is converted to a ProseMirror document
// by ../storage/convert.ts, which keeps macros, layouts and inline-comment markers.
//
// Authoring helpers, all written as storage elements the conversion keeps verbatim:
//  * `@name` mentions someone (`<ac:link><ri:user ri:account-id=…/></ac:link>`);
//  * `[[title` or Ctrl+K links a page (`<ac:link><ri:page ri:content-title=…/></ac:link>`),
//    or, in the Ctrl+K dialog, a web address;
//  * pasting, dropping or attaching a file uploads it to the page, then inserts it
//    (`<ac:image><ri:attachment …/></ac:image>` for images, a link otherwise).

import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState, type MutableRefObject, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Extension } from '@tiptap/core'
import { Plugin, PluginKey } from '@tiptap/pm/state'
import type { EditorView } from '@tiptap/pm/view'
import { EditorContent, useEditor, useEditorState, type Editor } from '@tiptap/react'
import {
  AtSign,
  Bold,
  Braces,
  Code,
  Globe,
  Info,
  Italic,
  Link2,
  List,
  ListOrdered,
  Minus,
  Paperclip,
  Quote,
  Redo2,
  Strikethrough,
  Table as TableIcon,
  Underline,
  Undo2,
  Unlink,
} from 'lucide-react'
import { toast, toastError } from '@/shell/actions'
import { Button, Field, IconButton, Input, Modal, Select, showMenuAt, type MenuEntry } from '@/ui'
import { confluenceApi, qk, type SearchHit } from '../api'
import { attachmentXml, docToStorage, inlineAtom, mentionXml, pageLinkXml, storageToDoc, type PMNode } from '../storage/convert'
import { editorExtensions } from '../storage/schema'
import { PagePicker } from './PagePicker'
import { SuggestMenu, usePageSearch, useUserSearch, type Suggestion } from './people'
import { asUrl, triggerBefore, type Trigger } from './triggers'
import { isImageFile, MAX_UPLOAD_BYTES, uploadKeepingBoth, uploadName } from './uploads'

export interface RichEditorHandle {
  /** The document as storage XHTML, right now. */
  getStorage: () => string
}

interface Props {
  storage: string
  pageId: string
  projectId: string | null
  /** The page's space (page links to other spaces name theirs). */
  spaceKey: string | null
  /** Names of the accounts the page mentions (for mention chips). */
  users: Record<string, string>
  onChange: (storage: string) => void
  /** Called once the document is loaded: the unchanged document as storage, and
   * whether the editor had to adjust structure it could not represent exactly. */
  onReady: (normalized: string, adjusted: boolean) => void
  onError: (e: unknown) => void
  /** Rendered above the document (the title field). */
  header?: ReactNode
}

const PANELS: { name: string; label: string }[] = [
  { name: 'info', label: 'Info panel' },
  { name: 'note', label: 'Note panel' },
  { name: 'warning', label: 'Warning panel' },
  { name: 'tip', label: 'Tip panel' },
  { name: 'expand', label: 'Expand' },
]

function blockValue(e: Editor): string {
  for (let l = 1; l <= 6; l++) if (e.isActive('heading', { level: l })) return `h${l}`
  if (e.isActive('codeBlock')) return 'code'
  return 'p'
}

// ---------------------------------------------------------------- suggestions

interface Active extends Trigger {
  /** Document position of the trigger's first character. */
  from: number
  at: { left: number; top: number; bottom: number }
}

/** What the editor plugin asks of the React side (kept in a ref, read synchronously). */
interface Controller {
  update: (view: EditorView) => void
  onKey: (e: KeyboardEvent) => boolean
  openLink: () => void
}

/** Watches the text before the caret for `@` / `[[`, routes keys to the open list, binds Ctrl+K. */
function suggestExtension(ctl: MutableRefObject<Controller>) {
  return Extension.create({
    name: 'cfSuggest',
    addKeyboardShortcuts() {
      return {
        'Mod-k': () => {
          ctl.current.openLink()
          return true
        },
      }
    },
    addProseMirrorPlugins() {
      return [
        new Plugin({
          key: new PluginKey('cfSuggest'),
          view: () => ({ update: (view) => ctl.current.update(view) }),
          props: { handleKeyDown: (_view, event) => ctl.current.onKey(event) },
        }),
      ]
    },
  })
}

function triggerAt(view: EditorView): Active | null {
  const { selection } = view.state
  if (!selection.empty) return null
  const $from = selection.$from
  if ($from.parent.type.spec.code || $from.marks().some((m) => m.type.name === 'code')) return null
  const start = Math.max(0, $from.parentOffset - 90)
  const text = $from.parent.textBetween(start, $from.parentOffset, undefined, '￼')
  const t = triggerBefore(text)
  if (!t) return null
  const from = $from.pos - t.length
  try {
    const c = view.coordsAtPos(from)
    return { ...t, from, at: { left: c.left, top: c.top, bottom: c.bottom } }
  } catch {
    return null
  }
}

// ---------------------------------------------------------------- link dialog

interface LinkRequest {
  /** The selected text (the link text), if any. */
  text: string
  /** The link under the caret, when editing one. */
  href: string | null
}

function LinkDialog({
  projectId,
  req,
  onClose,
  onUrl,
  onPage,
  onRemove,
}: {
  projectId: string | null
  req: LinkRequest
  onClose: () => void
  onUrl: (href: string, text: string) => void
  onPage: (hit: SearchHit, text: string) => void
  onRemove: () => void
}) {
  const [text, setText] = useState(req.text)
  const [page, setPage] = useState<SearchHit | null>(null)
  const [query, setQuery] = useState(req.href ?? '')
  const url = asUrl(query)
  return (
    <Modal
      title={req.href ? 'Edit link' : 'Insert link'}
      onClose={onClose}
      wide
      footer={
        <>
          {req.href && (
            <Button icon={Unlink} onClick={onRemove} style={{ marginRight: 'auto' }}>
              Remove link
            </Button>
          )}
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            disabled={!page && !url}
            onClick={() => (page ? onPage(page, text) : url && onUrl(url, text))}
          >
            {page ? 'Link to page' : 'Link'}
          </Button>
        </>
      }
    >
      <Field label="Link text" hint={req.text ? 'The selected text.' : 'Empty: the page title or address.'}>
        <Input value={text} onChange={(e) => setText(e.target.value)} placeholder="Text to show" />
      </Field>
      <Field label="Page or web address">
        <PagePicker
          projectId={projectId}
          value={page}
          onChange={setPage}
          autoFocus
          initialQuery={req.href ?? ''}
          placeholder="Search pages by title, or paste a https:// address"
          onEnter={(hit) => onPage(hit, text)}
          onQueryChange={setQuery}
          hideResults={!!url}
          onQueryEnter={(q) => {
            const u = asUrl(q)
            if (u && !page) {
              onUrl(u, text)
              return true
            }
            return false
          }}
          extra={(q) => {
            const u = asUrl(q)
            return u ? (
              <div className="cf-picker-list url">
                <div className="item active" onClick={() => onUrl(u, text)}>
                  <Globe size={13} className="wb-muted" />
                  <span className="wb-ellipsis">Link to {u}</span>
                  <span className="hint">Enter</span>
                </div>
              </div>
            ) : null
          }}
        />
      </Field>
    </Modal>
  )
}

// ---------------------------------------------------------------- toolbar

function Toolbar({ editor, onLink, onAttach, onMention }: { editor: Editor; onLink: () => void; onAttach: () => void; onMention: () => void }) {
  const s = useEditorState({
    editor,
    selector: ({ editor: e }) => ({
      block: blockValue(e),
      bold: e.isActive('bold'),
      italic: e.isActive('italic'),
      underline: e.isActive('underline'),
      strike: e.isActive('strike'),
      code: e.isActive('code'),
      bullet: e.isActive('bulletList'),
      ordered: e.isActive('orderedList'),
      quote: e.isActive('blockquote'),
      link: e.isActive('link'),
      table: e.isActive('table'),
      codeBlock: e.isActive('codeBlock'),
      language: (e.getAttributes('codeBlock').language as string | null) ?? '',
      canUndo: e.can().undo(),
      canRedo: e.can().redo(),
    }),
  })
  const c = () => editor.chain().focus()

  const setBlock = (v: string) => {
    if (v === 'p') c().setParagraph().run()
    else if (v === 'code') c().toggleCodeBlock().run()
    else c().setHeading({ level: Number(v[1]) as 1 | 2 | 3 | 4 | 5 | 6 }).run()
  }

  const tableMenu = (el: HTMLElement) => {
    const items: MenuEntry[] = s.table
      ? [
          { label: 'Add row above', run: () => c().addRowBefore().run() },
          { label: 'Add row below', run: () => c().addRowAfter().run() },
          { label: 'Add column left', run: () => c().addColumnBefore().run() },
          { label: 'Add column right', run: () => c().addColumnAfter().run() },
          'separator',
          { label: 'Toggle header row', run: () => c().toggleHeaderRow().run() },
          { label: 'Toggle header cell', run: () => c().toggleHeaderCell().run() },
          { label: 'Merge or split cells', run: () => c().mergeOrSplit().run() },
          'separator',
          { label: 'Delete row', danger: true, run: () => c().deleteRow().run() },
          { label: 'Delete column', danger: true, run: () => c().deleteColumn().run() },
          { label: 'Delete table', danger: true, run: () => c().deleteTable().run() },
        ]
      : [{ label: 'Insert 3 × 3 table', run: () => c().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run() }]
    showMenuAt(el, items)
  }

  const panelMenu = (el: HTMLElement) =>
    showMenuAt(
      el,
      PANELS.map((p) => ({
        label: p.label,
        run: () =>
          c()
            .insertContent({
              type: 'cfRich',
              attrs: { name: p.name, title: '', tpl: `<ac:structured-macro ac:name="${p.name}" ac:schema-version="1"><ac:rich-text-body>\u0000</ac:rich-text-body></ac:structured-macro>` },
              content: [{ type: 'paragraph' }],
            })
            .run(),
      })),
    )

  return (
    <div className="cf-edit-tools" onMouseDown={(e) => (e.target as HTMLElement).closest('button') && e.preventDefault()}>
      <IconButton size="small" icon={Undo2} label="Undo (Ctrl+Z)" disabled={!s.canUndo} onClick={() => c().undo().run()} />
      <IconButton size="small" icon={Redo2} label="Redo (Ctrl+Shift+Z)" disabled={!s.canRedo} onClick={() => c().redo().run()} />
      <span className="sep" />
      <Select value={s.block} onChange={(e) => setBlock(e.target.value)} aria-label="Block type">
        <option value="p">Paragraph</option>
        <option value="h1">Heading 1</option>
        <option value="h2">Heading 2</option>
        <option value="h3">Heading 3</option>
        <option value="h4">Heading 4</option>
        <option value="code">Code block</option>
      </Select>
      <IconButton size="small" icon={Bold} label="Bold (Ctrl+B)" active={s.bold} onClick={() => c().toggleBold().run()} />
      <IconButton size="small" icon={Italic} label="Italic (Ctrl+I)" active={s.italic} onClick={() => c().toggleItalic().run()} />
      <IconButton size="small" icon={Underline} label="Underline (Ctrl+U)" active={s.underline} onClick={() => c().toggleUnderline().run()} />
      <IconButton size="small" icon={Strikethrough} label="Strikethrough" active={s.strike} onClick={() => c().toggleStrike().run()} />
      <IconButton size="small" icon={Code} label="Inline code (Ctrl+E)" active={s.code} onClick={() => c().toggleMark('code').run()} />
      <span className="sep" />
      <IconButton size="small" icon={Link2} label="Link to a page or address (Ctrl+K, or type [[)" active={s.link} onClick={onLink} />
      <IconButton size="small" icon={AtSign} label="Mention someone (type @)" onClick={onMention} />
      <IconButton size="small" icon={Paperclip} label="Attach a file or image (or paste / drop it)" onClick={onAttach} />
      <span className="sep" />
      <IconButton size="small" icon={List} label="Bulleted list" active={s.bullet} onClick={() => c().toggleBulletList().run()} />
      <IconButton size="small" icon={ListOrdered} label="Numbered list" active={s.ordered} onClick={() => c().toggleOrderedList().run()} />
      <IconButton size="small" icon={Quote} label="Quote" active={s.quote} onClick={() => c().toggleBlockquote().run()} />
      <IconButton size="small" icon={Braces} label="Code block" active={s.codeBlock} onClick={() => c().toggleCodeBlock().run()} />
      <IconButton size="small" icon={TableIcon} label={s.table ? 'Table…' : 'Insert table'} active={s.table} onClick={(e) => tableMenu(e.currentTarget)} />
      <IconButton size="small" icon={Info} label="Insert panel…" onClick={(e) => panelMenu(e.currentTarget)} />
      <IconButton size="small" icon={Minus} label="Divider" onClick={() => c().setHorizontalRule().run()} />
      {s.codeBlock && (
        <>
          <span className="sep" />
          <span className="wb-xs wb-subtle" style={{ marginRight: 4 }}>
            Language
          </span>
          <Input
            small
            style={{ width: 110 }}
            value={s.language}
            placeholder="language"
            aria-label="Code language"
            onChange={(e) => editor.chain().updateAttributes('codeBlock', { language: e.target.value || null }).run()}
          />
        </>
      )}
    </div>
  )
}

// ---------------------------------------------------------------- editor

let uploadSeq = 0

const RichEditor = forwardRef<RichEditorHandle, Props>(function RichEditor(
  { storage, pageId, projectId, spaceKey, users, onChange, onReady, onError, header },
  ref,
) {
  const qc = useQueryClient()
  // The document is read once; the parent remounts (key) to load different storage.
  // Following the prop would rebuild the editor on every change it reports.
  const [initialStorage] = useState(storage)
  // Names of mentioned people, including the ones mentioned in this session.
  const names = useRef<Record<string, string>>({ ...users })
  const initial = useMemo<{ doc?: PMNode; error?: unknown }>(() => {
    try {
      return { doc: storageToDoc(initialStorage, { pageId, users: names.current }) }
    } catch (error) {
      return { error }
    }
  }, [initialStorage, pageId])
  const cb = useRef({ onChange, onReady, onError })
  cb.current = { onChange, onReady, onError }
  const timer = useRef<number | undefined>(undefined)
  const fileInput = useRef<HTMLInputElement>(null)

  const [active, setActive] = useState<Active | null>(null)
  const [index, setIndex] = useState(0)
  const [link, setLink] = useState<LinkRequest | null>(null)
  // A trigger dismissed with Escape stays closed until the caret leaves it.
  const dismissed = useRef<number | null>(null)
  const people = useUserSearch(projectId, active?.kind === 'mention' ? active.query : null)
  const pages = usePageSearch(projectId, active?.kind === 'page' ? active.query : null)
  const items: Suggestion[] = active
    ? active.kind === 'mention'
      ? (people.data ?? []).map((user) => ({ kind: 'user', user }))
      : (pages.data?.results ?? []).filter((p) => p.type === 'page').map((page) => ({ kind: 'page', page }))
    : []
  useEffect(() => setIndex(0), [active?.kind, active?.query, people.data, pages.data])

  const ctl = useRef<Controller>({ update: () => {}, onKey: () => false, openLink: () => {} })
  const extensions = useMemo(() => [...editorExtensions(), suggestExtension(ctl)], [])

  const editor = useEditor(
    {
      extensions,
      content: initial.doc ?? { type: 'doc', content: [{ type: 'paragraph' }] },
      immediatelyRender: true,
      enableContentCheck: true,
      onContentError: ({ error }) => cb.current.onError(error),
      editorProps: {
        attributes: { class: 'wb-prose wb-cf', spellcheck: 'true' },
        handlePaste: (_view, event) => {
          const files = Array.from(event.clipboardData?.files ?? [])
          // Text copied from a page may bring a picture of itself along: text wins.
          if (!files.length || event.clipboardData?.getData('text/plain')) return false
          event.preventDefault()
          void uploads.current(files, null)
          return true
        },
        handleDrop: (view, event, _slice, moved) => {
          const files = Array.from(event.dataTransfer?.files ?? [])
          if (moved || !files.length) return false
          event.preventDefault()
          const pos = view.posAtCoords({ left: event.clientX, top: event.clientY })?.pos ?? null
          void uploads.current(files, pos)
          return true
        },
      },
      onCreate: ({ editor: e }) => {
        if (!initial.doc) return
        const normalized = docToStorage(e.getJSON() as PMNode)
        cb.current.onReady(normalized, normalized !== docToStorage(initial.doc))
      },
      onUpdate: ({ editor: e }) => {
        window.clearTimeout(timer.current)
        timer.current = window.setTimeout(() => cb.current.onChange(docToStorage(e.getJSON() as PMNode)), 250)
      },
    },
    [initial],
  )

  // ------------------------------------------------ uploads

  const findPlaceholder = (token: string): number | null => {
    let at: number | null = null
    editor?.state.doc.descendants((node, pos) => {
      if (at !== null) return false
      if (node.type.name === 'cfInline' && node.attrs.kind === 'uploading' && node.attrs.preview === token) at = pos
      return true
    })
    return at
  }

  const uploads = useRef<(files: File[], pos: number | null) => Promise<void>>(async () => {})
  uploads.current = async (files: File[], pos: number | null) => {
    if (!editor) return
    const known = await qc.fetchQuery({ queryKey: qk.attachments(projectId, pageId), queryFn: () => confluenceApi.attachments(projectId, pageId), staleTime: 10_000 }).catch(() => null)
    const taken = (known?.attachments ?? []).map((a) => a.title)
    for (const file of files) {
      if (file.size > MAX_UPLOAD_BYTES) {
        toast('warning', `“${file.name}” is larger than ${MAX_UPLOAD_BYTES / 1024 / 1024} MB`)
        continue
      }
      const name = uploadName(file)
      const token = `upload-${++uploadSeq}`
      const placeholder = { type: 'cfInline', attrs: { xml: '', kind: 'uploading', label: `Uploading ${name}…`, preview: token } }
      const at = pos !== null && pos <= editor.state.doc.content.size ? pos : editor.state.selection.from
      editor.chain().focus().insertContentAt(at, placeholder).run()
      try {
        const att = await uploadKeepingBoth(projectId, pageId, file, name, taken, (sent, total) => {
          if (editor.isDestroyed) return
          const p = findPlaceholder(token)
          if (p === null || !total) return
          const label = `Uploading ${name}… ${Math.round((sent / total) * 100)}%`
          if (editor.state.doc.nodeAt(p)?.attrs.label !== label) editor.view.dispatch(editor.state.tr.setNodeAttribute(p, 'label', label))
        })
        taken.push(att.title)
        if (editor.isDestroyed) {
          toast('info', `Attached “${att.title}” to the page; the editor was closed before it could be inserted`)
          continue
        }
        const node = inlineAtom(attachmentXml(att.title, att.isImage || isImageFile(file)), { pageId })
        const p = findPlaceholder(token)
        if (p !== null) editor.chain().insertContentAt({ from: p, to: p + 1 }, node).run()
        else editor.chain().focus().insertContent(node).run()
      } catch (e) {
        const p = editor.isDestroyed ? null : findPlaceholder(token)
        if (p !== null) editor.chain().deleteRange({ from: p, to: p + 1 }).run()
        toastError(e, `Could not upload “${name}”`)
      }
    }
    void qc.invalidateQueries({ queryKey: qk.attachments(projectId, pageId) })
  }

  // ------------------------------------------------ suggestions and links

  const pick = (s: Suggestion) => {
    if (!editor || !active) return
    const to = editor.state.selection.from
    let node: PMNode
    if (s.kind === 'user') {
      names.current[s.user.accountId] = s.user.displayName
      node = inlineAtom(mentionXml(s.user.accountId), { users: names.current })
    } else {
      node = inlineAtom(pageLinkXml(s.page.title, s.page.spaceKey && s.page.spaceKey !== spaceKey ? s.page.spaceKey : null))
    }
    editor.chain().focus().deleteRange({ from: active.from, to }).insertContent([node, { type: 'text', text: ' ' }]).run()
    setActive(null)
  }

  const openLink = () => {
    if (!editor) return
    const { from, to, empty } = editor.state.selection
    const href = (editor.getAttributes('link').href as string | undefined) ?? null
    setLink({ text: empty ? '' : editor.state.doc.textBetween(from, to, ' ', ''), href })
  }

  ctl.current = {
    update: (view) => {
      const t = triggerAt(view)
      if (t && dismissed.current === t.from) return
      if (!t) dismissed.current = null
      setActive((cur) => (cur?.from === t?.from && cur?.query === t?.query && cur?.kind === t?.kind ? cur : t))
    },
    onKey: (e) => {
      if (!active) return false
      if (e.key === 'Escape') {
        dismissed.current = active.from
        setActive(null)
        return true
      }
      if (!items.length) return false
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        setIndex((i) => (i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length)
        return true
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        pick(items[Math.min(index, items.length - 1)])
        return true
      }
      return false
    },
    openLink,
  }

  const applyUrl = (href: string, text: string) => {
    if (!editor) return
    setLink(null)
    const c = editor.chain().focus()
    if (!editor.state.selection.empty || editor.isActive('link')) c.extendMarkRange('link').setLink({ href }).run()
    // The space after the link is plain text, so typing on does not extend the link.
    else c.insertContent([{ type: 'text', text: text || href, marks: [{ type: 'link', attrs: { href } }] }, { type: 'text', text: ' ' }]).unsetMark('link').run()
  }

  const applyPage = (hit: SearchHit, text: string) => {
    if (!editor) return
    setLink(null)
    const label = text.trim()
    const node = inlineAtom(pageLinkXml(hit.title, hit.spaceKey && hit.spaceKey !== spaceKey ? hit.spaceKey : null, label && label !== hit.title ? label : null))
    editor.chain().focus().deleteSelection().insertContent([node, { type: 'text', text: ' ' }]).run()
  }

  useEffect(() => {
    if (initial.error) cb.current.onError(initial.error)
  }, [initial])
  useEffect(() => () => window.clearTimeout(timer.current), [])

  useImperativeHandle(ref, () => ({ getStorage: () => (editor ? docToStorage(editor.getJSON() as PMNode) : initialStorage) }), [editor, initialStorage])

  if (!editor) return null
  const emptyText = active?.kind === 'mention' ? (active.query ? `No one named “${active.query}”` : 'Type a name') : active?.query ? 'No pages match' : 'Type a page title'
  return (
    <div className="cf-edit">
      <Toolbar
        editor={editor}
        onLink={openLink}
        onAttach={() => fileInput.current?.click()}
        onMention={() => editor.chain().focus().insertContent(editor.state.selection.$from.parentOffset > 0 ? ' @' : '@').run()}
      />
      <input
        ref={fileInput}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          const files = Array.from(e.target.files ?? [])
          e.target.value = ''
          if (files.length) void uploads.current(files, null)
        }}
      />
      <div className="cf-main">
        <div className="cf-doc cf-editor">
          {header}
          <EditorContent editor={editor} />
        </div>
      </div>
      {active && (
        <SuggestMenu
          at={active.at}
          items={items}
          active={index}
          loading={active.kind === 'mention' ? people.isFetching : pages.isFetching}
          empty={emptyText}
          onPick={pick}
          onHover={setIndex}
        />
      )}
      {link && (
        <LinkDialog
          projectId={projectId}
          req={link}
          onClose={() => {
            setLink(null)
            editor.commands.focus()
          }}
          onUrl={applyUrl}
          onPage={applyPage}
          onRemove={() => {
            setLink(null)
            editor.chain().focus().extendMarkRange('link').unsetLink().run()
          }}
        />
      )}
    </div>
  )
})

export default RichEditor
