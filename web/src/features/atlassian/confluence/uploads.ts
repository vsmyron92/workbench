// Attachment upload helpers: names for pasted images, free names next to existing
// attachments, the size cap, and an upload that retries under a free name when the
// name is taken (the editor never replaces someone's attachment by accident).

import { ApiError } from '@/api/client'
import { confluenceApi, type Attachment } from '../api'

/** Same cap as the server (Confluence Cloud's default attachment limit). */
export const MAX_UPLOAD_BYTES = 100 * 1024 * 1024

const EXT: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/svg+xml': 'svg',
  'image/bmp': 'bmp',
}

const pad = (n: number) => String(n).padStart(2, '0')

/**
 * The name to upload a file under. Clipboard images arrive as "image.png" (or
 * nameless); like Confluence they get a timestamped name so pastes do not collide.
 */
export function uploadName(file: { name: string; type: string }, now = new Date()): string {
  const generic = !file.name || /^image\.(png|jpe?g|gif|webp|bmp)$/i.test(file.name)
  if (!generic) return file.name.replace(/[\\/]/g, '_')
  const ext = file.name.split('.').pop()?.toLowerCase() || EXT[file.type] || 'png'
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  return `image-${stamp}.${ext}`
}

/** `name` if free, else `stem-1.ext`, `stem-2.ext`… */
export function freeName(name: string, taken: Iterable<string>): string {
  const set = new Set(taken)
  if (!set.has(name)) return name
  const dot = name.lastIndexOf('.')
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, '']
  for (let i = 1; i < 1000; i++) {
    const n = `${stem}-${i}${ext}`
    if (!set.has(n)) return n
  }
  return `${stem}-${Date.now()}${ext}`
}

export const isImageFile = (f: { type: string }) => f.type.startsWith('image/')

/**
 * Upload `file` to the page under `name`, or under a free variant when an attachment
 * of that name exists (409 `exists`). Never replaces.
 */
export async function uploadKeepingBoth(
  projectId: string | null,
  pageId: string,
  file: Blob,
  name: string,
  taken: string[],
  onProgress?: (sent: number, total: number) => void,
  signal?: AbortSignal,
): Promise<Attachment> {
  const tried = [...taken]
  let attempt = freeName(name, tried)
  for (let i = 0; i < 5; i++) {
    try {
      return await confluenceApi.upload(projectId, pageId, file, { name: attempt, onProgress, signal })
    } catch (e) {
      if (!(e instanceof ApiError && e.code === 'exists')) throw e
      tried.push(attempt)
      attempt = freeName(name, tried)
    }
  }
  throw new Error(`Could not find a free name for “${name}”`)
}
