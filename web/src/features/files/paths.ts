// Pure path helpers for the files slice. Project-relative paths use `/` and no
// leading slash (`''` is the project root) on every OS. Absolute paths (extra roots,
// files outside the project) are the server's: they start with `/`, and on a Windows
// server they are drive paths (`C:\…`, `C:/…`) that also separate with `\`. The
// server's OS comes from `GET /api/health`; until it arrives the Linux rules apply.

import { getHealth } from '@/api/health'
import type { FileContent } from './api'

const onWindows = () => getHealth()?.os === 'windows'

/** `C:\…` or `C:/…` (a drive's root included). */
const DRIVE_PATH = /^[A-Za-z]:[\\/]/

/**
 * Whether `p` is a Windows drive path (`C:\x`, `C:/x`): on a Windows server only. UNC
 * paths (`\\server\share`) are not: the server refuses network roots.
 */
export function isDrivePath(p: string): boolean {
  return onWindows() && DRIVE_PATH.test(p)
}

/** Whether `p` is absolute on the server: `/…` everywhere, a drive path on Windows. */
export function isAbsolutePath(p: string): boolean {
  return p.startsWith('/') || isDrivePath(p)
}

/** Index of the last separator: `/`, and on a Windows server also `\`. */
function lastSeparator(p: string): number {
  const i = p.lastIndexOf('/')
  return onWindows() ? Math.max(i, p.lastIndexOf('\\')) : i
}

export function basename(p: string): string {
  return p.slice(lastSeparator(p) + 1)
}

/** `a/b/c.rs` → `a/b`; `c.rs` → `''`; `/abs/x` → `/abs`; `/x` → `/`; on Windows `C:\x` → `C:\`. */
export function dirname(p: string): string {
  const i = lastSeparator(p)
  if (i < 0) return ''
  if (i === 0) return '/'
  if (i === 2 && isDrivePath(p)) return p.slice(0, 3)
  return p.slice(0, i)
}

/** The names of a path, root first (`/a/b` and `a/b` → `a`, `b`; on Windows `C:\a` → `C:`, `a`). */
export function segments(p: string): string[] {
  return p.split(onWindows() ? /[\\/]/ : '/').filter(Boolean)
}

/**
 * The project-relative `rel` (`/`-separated) under the absolute `root`, with the root's
 * own separator: `/home/u/p` + `src/a.rs` → `/home/u/p/src/a.rs`; on a Windows server
 * `C:\p` + `src/a.rs` → `C:\p\src\a.rs`.
 */
export function joinAbsolute(root: string, rel: string): string {
  if (!rel) return root
  if (onWindows() && (root.includes('\\') || !root.includes('/'))) return `${root.replace(/[\\/]$/, '')}\\${rel.replaceAll('/', '\\')}`
  return `${root.replace(/\/$/, '')}/${rel}`
}

/**
 * Whether two paths the server wrote name the same file: equal, or on a Windows server
 * equal up to `/` versus `\` and ASCII case (as the server compares them, `util::os::path`).
 */
export function samePath(a: string, b: string): boolean {
  if (a === b) return true
  if (!onWindows()) return false
  const norm = (p: string) => p.replaceAll('/', '\\').replace(/[A-Z]/g, (c) => c.toLowerCase())
  return norm(a) === norm(b)
}

/**
 * The path of a file's model URI (docs/ARCHITECTURE.md "Editor models"):
 * `/<projectId>/<path>`, or `/~abs<path>` for an absolute path. A drive path gets a `/`
 * before it (`/~abs/C:\x`); `parseModelUri` takes it off again.
 */
export function modelUriPath(projectId: string | null, path: string): string {
  return `/${projectId ?? '~abs'}${path.startsWith('/') ? path : '/' + path}`
}

export function joinPath(dir: string, name: string): string {
  if (!dir) return name
  return dir.endsWith('/') ? dir + name : `${dir}/${name}`
}

/** Lower-case extension without the dot (`''` for none; dotfiles have none). */
export function extname(p: string): string {
  const b = basename(p)
  const i = b.lastIndexOf('.')
  return i > 0 ? b.slice(i + 1).toLowerCase() : ''
}

/** `dir` is `path` or one of its ancestors (`''` contains everything). */
export function isWithin(dir: string, path: string): boolean {
  return dir === '' || path === dir || path.startsWith(dir + '/')
}

/** Every ancestor directory of a relative path, root first: `a/b/c` → `['', 'a', 'a/b']`. */
export function ancestors(path: string): string[] {
  const out = ['']
  const parts = path.split('/').filter(Boolean)
  for (let i = 1; i < parts.length; i++) out.push(parts.slice(0, i).join('/'))
  return out
}

/**
 * Resolve a link found in a document at `fromFile` (relative to the project root,
 * or absolute). `/x` means the project root for relative documents. Returns null
 * for links that would climb above the root. A document at a drive path keeps its
 * drive and separator (`C:\notes\plan.md` + `img/a.png` → `C:\notes\img\a.png`).
 */
export function resolveLink(fromFile: string, href: string): string | null {
  const absolute = isAbsolutePath(fromFile)
  const root = isDrivePath(fromFile) ? fromFile.slice(0, 3) : absolute ? '/' : ''
  const sep = root.endsWith('\\') ? '\\' : '/'
  let parts: string[]
  if (href.startsWith('/') && !absolute) parts = []
  else parts = segments(dirname(fromFile).slice(root.length))
  for (const seg of href.split('/')) {
    if (seg === '' || seg === '.') continue
    if (seg === '..') {
      if (!parts.length) return null
      parts.pop()
    } else {
      parts.push(decodeURIComponentSafe(seg))
    }
  }
  return root + parts.join(sep)
}

function decodeURIComponentSafe(s: string): string {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

/** Split `path#L10` / `path#L10-L20` / `path#anchor`. */
export function splitAnchor(href: string): { path: string; line?: number; anchor?: string } {
  const i = href.indexOf('#')
  if (i < 0) return { path: href }
  const path = href.slice(0, i)
  const frag = href.slice(i + 1)
  const m = /^L(\d+)/i.exec(frag)
  return m ? { path, line: Number(m[1]) } : { path, anchor: frag }
}

/** Whether an href points outside the app (http:, mailto:, …). */
export function isExternalHref(href: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('//')
}

// ---------------------------------------------------------------- panel ids

export const editorPanelId = (projectId: string | null, path: string) => `editor:${projectId ?? ''}:${path}`
export const markdownPanelId = (projectId: string | null, path: string) => `markdown:${projectId ?? ''}:${path}`
export const searchPanelId = (projectId: string) => `search:${projectId}`

/** Buffer / model key shared by every editor of one file. */
export const bufferKey = (projectId: string | null, path: string) => `${projectId ?? '~'}:${path}`

const GENERIC_NAMES = new Set([
  'mod.rs', 'lib.rs', 'main.rs', 'build.rs', 'index.ts', 'index.tsx', 'index.js', 'index.html', '__init__.py',
  'cargo.toml', 'package.json', 'readme.md', 'claude.md', 'dockerfile', 'makefile', 'tsconfig.json', 'types.ts',
])

/** Tab title: the file name, with its folder for names that are everywhere (`files/mod.rs`). */
export function tabTitle(path: string): string {
  const name = basename(path)
  const dir = basename(dirname(path))
  return GENERIC_NAMES.has(name.toLowerCase()) && dir && dir !== '/' ? `${dir}/${name}` : name
}

// ---------------------------------------------------------------- viewers

export type MediaKind = 'image' | 'video' | 'audio' | 'pdf'
const IMAGE = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico', 'avif'])
const VIDEO = new Set(['mp4', 'webm', 'mov', 'm4v', 'ogv', 'mkv'])
const AUDIO = new Set(['mp3', 'wav', 'ogg', 'oga', 'flac', 'm4a', 'aac', 'opus'])

/** Files shown with a media viewer instead of the text editor. SVG is text (with a preview). */
export function mediaKind(path: string): MediaKind | null {
  const e = extname(path)
  if (IMAGE.has(e)) return 'image'
  if (VIDEO.has(e)) return 'video'
  if (AUDIO.has(e)) return 'audio'
  if (e === 'pdf') return 'pdf'
  return null
}

export const isMarkdown = (path: string) => ['md', 'markdown', 'mdx'].includes(extname(path))
export const isSvg = (path: string) => extname(path) === 'svg'

export type ViewerKind = 'text' | MediaKind | 'binary' | 'tooLarge' | 'sensitive'

export function viewerFor(path: string, meta: Pick<FileContent, 'content' | 'binary' | 'tooLarge' | 'sensitive' | 'etag'>): ViewerKind {
  const media = mediaKind(path)
  if (meta.sensitive && meta.content === null && !meta.binary && !meta.tooLarge && meta.etag === null) return 'sensitive'
  if (media) return media
  if (meta.tooLarge) return 'tooLarge'
  if (meta.binary || meta.content === null) return 'binary'
  return 'text'
}

// ---------------------------------------------------------------- quick open

/** `main.rs:12:5` → query + line/column; `:12` → go to line in the active editor. */
export function parseGoto(input: string): { query: string; line?: number; column?: number } {
  const m = /^(.*?):(\d+)(?::(\d+))?\s*$/.exec(input.trim())
  if (!m) return { query: input.trim() }
  return { query: m[1].trim(), line: Number(m[2]), column: m[3] ? Number(m[3]) : undefined }
}

// ---------------------------------------------------------------- highlight.js names (mobile viewer)

const HLJS: Record<string, string> = {
  rs: 'rust', ts: 'typescript', tsx: 'typescript', mts: 'typescript', js: 'javascript', jsx: 'javascript', mjs: 'javascript',
  cjs: 'javascript', json: 'json', cs: 'csharp', c: 'c', h: 'c', cc: 'cpp', cpp: 'cpp', cxx: 'cpp', 'c++': 'cpp', hpp: 'cpp',
  hh: 'cpp', hxx: 'cpp', 'h++': 'cpp', ipp: 'cpp', tpp: 'cpp', txx: 'cpp', inl: 'cpp', ixx: 'cpp', cppm: 'cpp', ino: 'cpp',
  cu: 'cpp', cuh: 'cpp', v: 'verilog', vh: 'verilog', sv: 'verilog', svh: 'verilog', vhd: 'vhdl', vhdl: 'vhdl', vho: 'vhdl',
  vht: 'vhdl', go: 'go', java: 'java', kt: 'kotlin', py: 'python', rb: 'ruby', sh: 'bash', bash: 'bash', zsh: 'bash', yml: 'yaml', yaml: 'yaml', toml: 'ini',
  ini: 'ini', md: 'markdown', sql: 'sql', html: 'xml', xml: 'xml', svg: 'xml', css: 'css', scss: 'scss', less: 'less',
  dockerfile: 'dockerfile', makefile: 'makefile', lua: 'lua', php: 'php', swift: 'swift', diff: 'diff', patch: 'diff',
}

export function hljsLanguage(path: string): string {
  const name = basename(path).toLowerCase()
  if (name === 'dockerfile' || name.startsWith('dockerfile.')) return 'dockerfile'
  if (name === 'makefile') return 'makefile'
  return HLJS[extname(path)] ?? ''
}

/** A Markdown code fence that cannot be closed early by the content. */
export function fenced(content: string, lang: string): string {
  const longest = Math.max(0, ...Array.from(content.matchAll(/`+/g), (m) => m[0].length))
  const fence = '`'.repeat(Math.max(3, longest + 1))
  return `${fence}${lang}\n${content}\n${fence}\n`
}
