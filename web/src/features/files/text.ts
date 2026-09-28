// Small text helpers shared by the editor buffers and viewers.

/**
 * The smallest single replacement turning `a` into `b`: `a.slice(start, end)` is
 * replaced by `text`. Applying only this keeps cursors and scroll positions
 * outside the changed region untouched when a file is reloaded from disk.
 */
export function minimalEdit(a: string, b: string): { start: number; end: number; text: string } | null {
  if (a === b) return null
  const max = Math.min(a.length, b.length)
  let pre = 0
  while (pre < max && a.charCodeAt(pre) === b.charCodeAt(pre)) pre++
  // Do not split a surrogate pair.
  if (pre > 0 && isHighSurrogate(a.charCodeAt(pre - 1))) pre--
  let suf = 0
  while (suf < max - pre && a.charCodeAt(a.length - 1 - suf) === b.charCodeAt(b.length - 1 - suf)) suf++
  if (suf > 0 && isLowSurrogate(a.charCodeAt(a.length - suf))) suf--
  return { start: pre, end: a.length - suf, text: b.slice(pre, b.length - suf) }
}

function isHighSurrogate(c: number) {
  return c >= 0xd800 && c <= 0xdbff
}
function isLowSurrogate(c: number) {
  return c >= 0xdc00 && c <= 0xdfff
}

/** `LF`, `CRLF` or `Mixed`, from the first few thousand line breaks. */
export function eolOf(text: string): 'LF' | 'CRLF' | 'Mixed' | null {
  let crlf = 0
  let lf = 0
  const limit = Math.min(text.length, 2_000_000)
  for (let i = 0; i < limit; i++) {
    if (text.charCodeAt(i) === 10) {
      if (i > 0 && text.charCodeAt(i - 1) === 13) crlf++
      else lf++
      if (crlf + lf > 5000) break
    }
  }
  if (!crlf && !lf) return null
  if (crlf && lf) return 'Mixed'
  return crlf ? 'CRLF' : 'LF'
}

/** Copy text; falls back to a hidden textarea where the Clipboard API is unavailable (plain HTTP on a LAN). */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    /* fall through */
  }
  const ta = document.createElement('textarea')
  ta.value = text
  ta.style.position = 'fixed'
  ta.style.opacity = '0'
  document.body.appendChild(ta)
  ta.select()
  let ok = false
  try {
    ok = document.execCommand('copy')
  } catch {
    ok = false
  }
  ta.remove()
  return ok
}

/** GitHub-style heading slug (for `#anchor` links in rendered Markdown). */
export function slugify(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, '')
    .replace(/\s/g, '-')
}
