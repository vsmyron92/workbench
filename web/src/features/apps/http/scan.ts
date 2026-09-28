// Where the requests of an .http file start (for the editor's Send Request marks),
// by the rules the server parses them with (server/src/apps/http_client.rs): blocks
// between `###` lines; in each, the first line that is not blank, a comment or an
// `@variable = value` line is the request line.

export interface RequestMark {
  /** 1-based. */
  line: number
  method: string
}

const METHODS = new Set(['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS', 'TRACE', 'CONNECT'])

export function scanRequests(text: string): RequestMark[] {
  const lines = text.split(/\r?\n/)
  const out: RequestMark[] = []
  let looking = true
  lines.forEach((raw, i) => {
    const l = raw.trim()
    if (l.startsWith('###')) {
      looking = true
      return
    }
    if (!looking || !l || l.startsWith('#') || l.startsWith('//') || /^@[\w.-]+\s*=/.test(l)) return
    const first = l.split(/\s+/)[0].toUpperCase()
    out.push({ line: i + 1, method: METHODS.has(first) ? first : 'GET' })
    looking = false
  })
  return out
}
