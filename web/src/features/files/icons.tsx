// File-type icons (lucide) tinted with theme tokens, JetBrains style: the icon
// says what a file is, the name's colour says its VCS status.

import type { ComponentType } from 'react'
import {
  BookOpen,
  Braces,
  Database,
  File,
  FileArchive,
  FileAudio,
  FileCode,
  FileCog,
  FileImage,
  FileLock,
  FileSpreadsheet,
  FileSymlink,
  FileTerminal,
  FileText,
  FileType,
  FileVideo,
  Folder,
  FolderOpen,
  Package,
} from 'lucide-react'
import { basename, extname } from './paths'

type Icon = ComponentType<{ size?: number; className?: string }>

interface IconSpec {
  icon: Icon
  /** CSS class setting the tint from tokens (files.css). */
  tone: 'code' | 'config' | 'doc' | 'media' | 'data' | 'script' | 'muted' | 'lock' | 'folder'
}

const BY_NAME: Record<string, IconSpec> = {
  'cargo.toml': { icon: Package, tone: 'config' },
  'cargo.lock': { icon: FileLock, tone: 'muted' },
  'package.json': { icon: Package, tone: 'config' },
  'package-lock.json': { icon: FileLock, tone: 'muted' },
  dockerfile: { icon: FileCog, tone: 'config' },
  makefile: { icon: FileTerminal, tone: 'script' },
  '.gitignore': { icon: FileCog, tone: 'muted' },
  '.gitlab-ci.yml': { icon: FileCog, tone: 'config' },
  license: { icon: FileText, tone: 'doc' },
}

const CODE = new Set(['rs', 'ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'mts', 'cts', 'cs', 'c', 'h', 'cc', 'cpp', 'cxx', 'c++', 'hpp', 'hh', 'hxx', 'h++', 'ipp', 'tpp', 'inl', 'ino', 'cu', 'cuh', 'v', 'vh', 'sv', 'svh', 'vhd', 'vhdl', 'go', 'java', 'kt', 'kts', 'py', 'rb', 'php', 'swift', 'lua', 'html', 'htm', 'css', 'scss', 'less', 'vue', 'svelte', 'shader', 'hlsl', 'glsl', 'uss', 'uxml'])
const CONFIG = new Set(['toml', 'yml', 'yaml', 'ini', 'cfg', 'conf', 'env', 'properties', 'editorconfig', 'csproj', 'sln', 'asmdef', 'xml', 'plist', 'lock'])
const DATA = new Set(['json', 'jsonc', 'json5', 'jsonl', 'ndjson'])
const TABLE = new Set(['csv', 'tsv', 'xlsx', 'xls', 'ods'])
const DB = new Set(['sql', 'db', 'sqlite', 'sqlite3'])
const DOC = new Set(['md', 'markdown', 'mdx', 'txt', 'rst', 'adoc', 'org'])
const IMAGE = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico', 'avif', 'svg', 'psd', 'tga', 'exr'])
const VIDEO = new Set(['mp4', 'webm', 'mov', 'm4v', 'ogv', 'mkv'])
const AUDIO = new Set(['mp3', 'wav', 'ogg', 'oga', 'flac', 'm4a', 'aac', 'opus'])
const ARCHIVE = new Set(['zip', 'gz', 'tgz', 'bz2', 'xz', 'zst', '7z', 'rar', 'tar', 'jar', 'unitypackage'])
const SCRIPT = new Set(['sh', 'bash', 'zsh', 'fish', 'ps1', 'bat', 'cmd'])
const FONT = new Set(['ttf', 'otf', 'woff', 'woff2'])

export function fileIconSpec(path: string, opts: { dir?: boolean; open?: boolean; symlink?: boolean; sensitive?: boolean } = {}): IconSpec {
  if (opts.dir) return { icon: opts.open ? FolderOpen : Folder, tone: 'folder' }
  if (opts.sensitive) return { icon: FileLock, tone: 'lock' }
  if (opts.symlink) return { icon: FileSymlink, tone: 'muted' }
  const name = basename(path).toLowerCase()
  const byName = BY_NAME[name]
  if (byName) return byName
  if (name.startsWith('.env')) return { icon: FileLock, tone: 'lock' }
  if (name.startsWith('dockerfile')) return { icon: FileCog, tone: 'config' }
  if (name === 'readme.md' || name === 'claude.md' || name === 'agents.md') return { icon: BookOpen, tone: 'doc' }
  const e = extname(path)
  if (CODE.has(e)) return { icon: FileCode, tone: 'code' }
  if (DATA.has(e)) return { icon: Braces, tone: 'data' }
  if (CONFIG.has(e)) return { icon: FileCog, tone: 'config' }
  if (DOC.has(e)) return { icon: FileText, tone: 'doc' }
  if (IMAGE.has(e)) return { icon: FileImage, tone: 'media' }
  if (VIDEO.has(e)) return { icon: FileVideo, tone: 'media' }
  if (AUDIO.has(e)) return { icon: FileAudio, tone: 'media' }
  if (ARCHIVE.has(e)) return { icon: FileArchive, tone: 'muted' }
  if (SCRIPT.has(e)) return { icon: FileTerminal, tone: 'script' }
  if (TABLE.has(e)) return { icon: FileSpreadsheet, tone: 'data' }
  if (DB.has(e)) return { icon: Database, tone: 'data' }
  if (FONT.has(e)) return { icon: FileType, tone: 'muted' }
  if (e === 'pdf') return { icon: FileText, tone: 'media' }
  return { icon: File, tone: 'muted' }
}

export function FileIcon(props: { path: string; dir?: boolean; open?: boolean; symlink?: boolean; sensitive?: boolean; size?: number }) {
  const spec = fileIconSpec(props.path, props)
  const I = spec.icon
  return <I size={props.size ?? 15} className={`wb-ficon ${spec.tone}`} />
}
