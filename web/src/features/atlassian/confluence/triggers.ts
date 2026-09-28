// What the text before the caret asks the rich editor to suggest: `@name` offers
// people to mention, `[[title` offers pages to link. Atoms (mentions, links, images)
// appear in the text as U+FFFC and end a trigger.

export interface Trigger {
  kind: 'mention' | 'page'
  query: string
  /** Characters the trigger spans before the caret (`@` or `[[` included). */
  length: number
}

const MENTION = /(?:^|[\s(])@([^\s@￼]{0,30})$/u
const PAGE = /\[\[([^\]\n￼]{0,80})$/u

export function triggerBefore(text: string): Trigger | null {
  const p = PAGE.exec(text)
  if (p) return { kind: 'page', query: p[1], length: p[1].length + 2 }
  const m = MENTION.exec(text)
  if (m) return { kind: 'mention', query: m[1], length: m[1].length + 1 }
  return null
}

/** A web address typed into the link picker (https://…, or a bare domain). */
export function asUrl(text: string): string | null {
  const t = text.trim()
  if (/^https?:\/\/\S+$/i.test(t)) return t
  if (/^mailto:\S+@\S+$/i.test(t)) return t
  if (/^(www\.)?[a-z0-9-]+(\.[a-z0-9-]+)+(\/\S*)?$/i.test(t) && !/\s/.test(t)) return `https://${t}`
  return null
}
