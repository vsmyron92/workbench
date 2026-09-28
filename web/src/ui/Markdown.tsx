// GitHub-flavoured markdown rendered as a readable page: tables, task lists,
// footnotes, GitHub alerts (`> [!NOTE]`), heading anchors, code blocks with a
// language label and a copy button, Mermaid diagrams, and the HTML GitHub allows
// in markdown (details/summary, kbd, sub/sup, images with a size…).
//
// Repository markdown is untrusted: raw HTML goes through rehype-sanitize with
// GitHub's schema (no scripts, event handlers, styles or javascript: URLs; ids
// prefixed so they cannot clobber globals), and Mermaid runs with
// securityLevel "strict". `#anchor` links scroll within the document; other
// links open in a new window unless `onLinkClick` takes them (relative repo
// links). Token colours come from theme/global.css (.hljs-*). Loaded lazily
// through the `@/ui` barrel.

import { createContext, useContext, useEffect, useId, useMemo, useRef, useState, type ComponentProps, type ReactNode, type RefObject } from 'react'
import ReactMarkdown, { type Components, type ExtraProps } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import rehypeRaw from 'rehype-raw'
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize'
import rehypeSlug from 'rehype-slug'
import rehypeHighlight from 'rehype-highlight'
import { Check, Copy, Info, Lightbulb, Link2, MessageSquareWarning, OctagonAlert, TriangleAlert, type LucideIcon } from 'lucide-react'
import bash from 'highlight.js/lib/languages/bash'
import csharp from 'highlight.js/lib/languages/csharp'
import css from 'highlight.js/lib/languages/css'
import diff from 'highlight.js/lib/languages/diff'
import dockerfile from 'highlight.js/lib/languages/dockerfile'
import go from 'highlight.js/lib/languages/go'
import ini from 'highlight.js/lib/languages/ini'
import java from 'highlight.js/lib/languages/java'
import javascript from 'highlight.js/lib/languages/javascript'
import json from 'highlight.js/lib/languages/json'
import markdown from 'highlight.js/lib/languages/markdown'
import python from 'highlight.js/lib/languages/python'
import rust from 'highlight.js/lib/languages/rust'
import shell from 'highlight.js/lib/languages/shell'
import sql from 'highlight.js/lib/languages/sql'
import typescript from 'highlight.js/lib/languages/typescript'
import xml from 'highlight.js/lib/languages/xml'
import yaml from 'highlight.js/lib/languages/yaml'
import { useUi } from '@/state/store'
import { alertTitle, codeLanguage, findAnchor, hastText, rehypeAlerts, splitFrontmatter, type HastNode } from './markdownPlugins'

/** Languages worth highlighting in project docs (TOML reads well as `ini`). */
const LANGUAGES = { bash, csharp, css, diff, dockerfile, go, ini, java, javascript, json, markdown, python, rust, shell, sql, typescript, xml, yaml }
const ALIASES = { bash: ['sh', 'zsh', 'console'], ini: ['toml'], javascript: ['js', 'jsx'], typescript: ['ts', 'tsx'], xml: ['html', 'svg'], yaml: ['yml'] }

/** GitHub's schema, plus the `data-footnote*` attributes remark-gfm emits. */
const SCHEMA = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    '*': [...(defaultSchema.attributes?.['*'] ?? []), 'dataFootnotes', 'dataFootnoteRef', 'dataFootnoteBackref'],
  },
}

const ALERT_ICONS: Record<string, LucideIcon> = { note: Info, tip: Lightbulb, important: MessageSquareWarning, warning: TriangleAlert, caution: OctagonAlert }

const REMARK = [remarkGfm]
// Footnote ids come out unprefixed so the sanitizer's `user-content-` prefix is
// the only one (links keep `#fn-1`; findAnchor resolves both).
const REMARK_REHYPE = { clobberPrefix: '' }
const REHYPE = [
  rehypeRaw,
  [rehypeSanitize, SCHEMA],
  // After sanitizing: these ids and classes are ours, not the document's.
  [rehypeSlug, { prefix: 'md-' }],
  rehypeAlerts,
  [rehypeHighlight, { detect: false, ignoreMissing: true, languages: LANGUAGES, aliases: ALIASES }],
] as const

export function Markdown({
  text,
  onLinkClick,
  resolveImage,
  className,
}: {
  text: string
  /** Return true to prevent default navigation (e.g. open a repo file in the editor). */
  onLinkClick?: (href: string) => boolean
  /** Map a relative image src to a URL (e.g. the files raw endpoint). */
  resolveImage?: (src: string) => string
  className?: string
}) {
  const { frontmatter, body } = splitFrontmatter(text)
  const root = useRef<HTMLDivElement>(null)
  // Click handlers read the latest callbacks through a ref, so the parsed tree
  // below only changes with the text (a re-render of the parent, e.g. on every
  // scroll or keystroke, neither re-parses nor remounts diagrams).
  const handlers = useRef<Handlers>({ jump: () => undefined })
  useEffect(() => {
    handlers.current = {
      onLinkClick,
      /** Scroll to an in-document anchor; the caller's handler gets the ones not found. */
      jump: (href) => {
        const el = root.current && findAnchor(root.current, href)
        if (el) el.scrollIntoView({ block: 'start', behavior: 'smooth' })
        else onLinkClick?.(href)
      },
    }
  })
  const tree = useMemo(
    () => (
      <ReactMarkdown remarkPlugins={REMARK as never} remarkRehypeOptions={REMARK_REHYPE} rehypePlugins={REHYPE as never} components={COMPONENTS}>
        {body}
      </ReactMarkdown>
    ),
    [body],
  )
  return (
    <div ref={root} className={['wb-prose', className].filter(Boolean).join(' ')}>
      {frontmatter !== null && (
        <details className="wb-md-frontmatter">
          <summary>Front matter</summary>
          <pre>
            <code>{frontmatter}</code>
          </pre>
        </details>
      )}
      <HandlersContext.Provider value={handlers}>
        <ImageContext.Provider value={resolveImage}>{tree}</ImageContext.Provider>
      </HandlersContext.Provider>
    </div>
  )
}

interface Handlers {
  onLinkClick?: (href: string) => boolean
  jump: (href: string) => void
}
const HandlersContext = createContext<RefObject<Handlers> | null>(null)
const ImageContext = createContext<((src: string) => string) | undefined>(undefined)

// Module-level so their identity is stable: an inline map would be a new set of
// component types on every render, and React would remount the whole document.
const COMPONENTS: Components = {
  a: Anchor,
  img: Image,
  h1: ({ id, children }) => <Heading level={1} id={id}>{children}</Heading>,
  h2: ({ id, children }) => <Heading level={2} id={id}>{children}</Heading>,
  h3: ({ id, children }) => <Heading level={3} id={id}>{children}</Heading>,
  h4: ({ id, children }) => <Heading level={4} id={id}>{children}</Heading>,
  blockquote: ({ node, className, children }) => {
    const kind = (node as HastNode | undefined)?.properties?.dataAlert
    if (typeof kind !== 'string') return <blockquote className={className}>{children}</blockquote>
    const Icon = ALERT_ICONS[kind] ?? Info
    return (
      <blockquote className={className}>
        <div className="wb-alert-title">
          <Icon size={15} />
          {alertTitle(kind)}
        </div>
        {children}
      </blockquote>
    )
  },
  pre: ({ node, children }) => {
    const code = (node as HastNode | undefined)?.children?.find((c) => c.type === 'element' && c.tagName === 'code')
    const lang = codeLanguage(code?.properties?.className)
    const source = hastText(code)
    if (lang === 'mermaid') return <Mermaid code={source} />
    return (
      <CodeBlock lang={lang} source={source}>
        {children}
      </CodeBlock>
    )
  },
  table: ({ children }) => (
    <div className="wb-md-table">
      <table>{children}</table>
    </div>
  ),
}

/** `#anchor` links scroll within the document; others open in a new window unless the caller takes them. */
function Anchor({ node: _node, href, children, ...rest }: ComponentProps<'a'> & ExtraProps) {
  const handlers = useContext(HandlersContext)
  const local = href?.startsWith('#')
  return (
    <a
      {...rest}
      href={href}
      target={local ? undefined : '_blank'}
      rel={local ? undefined : 'noopener noreferrer'}
      onClick={(e) => {
        const h = handlers?.current
        if (!href || !h) return
        if (local) {
          e.preventDefault()
          h.jump(href)
        } else if (h.onLinkClick?.(href)) e.preventDefault()
      }}
    >
      {children}
    </a>
  )
}

function Image({ src, alt, width, height, title }: ComponentProps<'img'> & ExtraProps) {
  const resolve = useContext(ImageContext)
  return <img src={typeof src === 'string' && resolve ? resolve(src) : (src as string)} alt={alt ?? ''} title={title} width={width} height={height} loading="lazy" />
}

/** A heading with a hover anchor (`#`) that links to itself. */
function Heading({ level, id, children }: { level: 1 | 2 | 3 | 4; id?: string; children: ReactNode }) {
  const handlers = useContext(HandlersContext)
  const Tag = `h${level}` as const
  const slug = id?.replace(/^md-/, '')
  return (
    <Tag id={id} className="wb-md-heading">
      {children}
      {slug && (
        <a
          className="wb-md-anchor"
          href={`#${slug}`}
          aria-label="Link to this section"
          onClick={(e) => {
            e.preventDefault()
            handlers?.current.jump(`#${slug}`)
          }}
        >
          <Link2 size={14} />
        </a>
      )}
    </Tag>
  )
}

function CodeBlock({ lang, source, children }: { lang: string | null; source: string; children: ReactNode }) {
  const [copied, setCopied] = useState(false)
  const copy = () => {
    void navigator.clipboard?.writeText(source).then(
      () => {
        setCopied(true)
        window.setTimeout(() => setCopied(false), 1500)
      },
      () => undefined,
    )
  }
  return (
    <div className="wb-md-code">
      <div className="wb-md-code-bar">
        <span>{lang ?? 'text'}</span>
        <button type="button" onClick={copy} aria-label="Copy code" title="Copy">
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </button>
      </div>
      <pre>{children}</pre>
    </div>
  )
}

/** A Mermaid diagram, rendered by the lazily loaded library in strict mode. */
function Mermaid({ code }: { code: string }) {
  const theme = useUi((s) => s.prefs.theme)
  const id = `wb-mermaid-${useId().replace(/[^a-zA-Z0-9]/g, '')}`
  const [svg, setSvg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const mermaid = (await import('mermaid')).default
        mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: theme === 'dark' ? 'dark' : 'default' })
        const out = await mermaid.render(id, code)
        if (!cancelled) {
          setSvg(out.svg)
          setError(null)
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      }
    })()
    return () => {
      cancelled = true
    }
  }, [code, id, theme])
  if (error) {
    return (
      <div className="wb-md-code">
        <div className="wb-md-code-bar">
          <span>mermaid · could not render: {error.split('\n')[0]}</span>
        </div>
        <pre>
          <code>{code}</code>
        </pre>
      </div>
    )
  }
  // Mermaid's strict mode encodes labels and disables click handlers; its output
  // is its own sanitized SVG.
  return <div className="wb-md-mermaid" dangerouslySetInnerHTML={svg ? { __html: svg } : undefined} />
}

export default Markdown
