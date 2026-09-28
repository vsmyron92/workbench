// The Database feature's pure parts: splitting SQL into statements (the one at the
// caret is what Ctrl+Enter runs), identifiers, and copying results.

export interface Statement {
  /** Offsets in the text: [start, end). `text` is trimmed of the `;` and outer space. */
  start: number
  end: number
  text: string
}

/**
 * PostgreSQL statements of `sql`, split at top-level `;`: not inside quotes
 * ('…' with '' escapes, E'…' with \ escapes, "…"), dollar-quoted bodies ($$…$$,
 * $tag$…$tag$), line comments (--) or nested block comments.
 */
export function splitStatements(sql: string): Statement[] {
  const out: Statement[] = []
  let start = 0
  let i = 0
  const n = sql.length
  const push = (end: number) => {
    const raw = sql.slice(start, end)
    const lead = raw.length - raw.trimStart().length
    const text = raw.trim()
    if (text) out.push({ start: start + lead, end: start + lead + text.length, text })
  }
  while (i < n) {
    const c = sql[i]
    const next = sql[i + 1]
    if (c === '-' && next === '-') {
      const nl = sql.indexOf('\n', i)
      i = nl < 0 ? n : nl + 1
    } else if (c === '/' && next === '*') {
      let depth = 1
      i += 2
      while (i < n && depth > 0) {
        if (sql[i] === '/' && sql[i + 1] === '*') {
          depth++
          i += 2
        } else if (sql[i] === '*' && sql[i + 1] === '/') {
          depth--
          i += 2
        } else i++
      }
    } else if (c === "'") {
      const escapes = i > 0 && /[eE]/.test(sql[i - 1]) && (i < 2 || !/[\w$]/.test(sql[i - 2]))
      i++
      while (i < n) {
        if (escapes && sql[i] === '\\') i += 2
        else if (sql[i] === "'" && sql[i + 1] === "'") i += 2
        else if (sql[i] === "'") {
          i++
          break
        } else i++
      }
    } else if (c === '"') {
      i++
      while (i < n) {
        if (sql[i] === '"' && sql[i + 1] === '"') i += 2
        else if (sql[i] === '"') {
          i++
          break
        } else i++
      }
    } else if (c === '$' && (i === 0 || !/[\w$]/.test(sql[i - 1]))) {
      const m = /^\$([A-Za-z_][A-Za-z0-9_]*)?\$/.exec(sql.slice(i))
      if (m) {
        const tag = m[0]
        const close = sql.indexOf(tag, i + tag.length)
        i = close < 0 ? n : close + tag.length
      } else i++
    } else if (c === ';') {
      push(i)
      start = i + 1
      i++
    } else i++
  }
  push(n)
  return out
}

/**
 * The statement Ctrl+Enter runs for a caret at `offset`: the one containing it, else
 * the nearest one before (the caret just after a `;` or on a blank line below).
 */
export function statementAt(sql: string, offset: number): Statement | null {
  const all = splitStatements(sql)
  if (!all.length) return null
  const inside = all.find((s) => offset >= s.start && offset <= s.end + 1)
  if (inside) return inside
  const before = all.filter((s) => s.end <= offset)
  return before.length ? before[before.length - 1] : all[0]
}

/** A quoted identifier when it needs quotes (`Order Items` → `"Order Items"`). */
export function quoteIdent(name: string): string {
  return /^[a-z_][a-z0-9_$]*$/.test(name) && !RESERVED.has(name) ? name : `"${name.replace(/"/g, '""')}"`
}

const RESERVED = new Set(['all', 'and', 'any', 'array', 'as', 'asc', 'case', 'check', 'column', 'constraint', 'create', 'default', 'desc', 'distinct', 'do', 'else', 'end', 'except', 'false', 'for', 'foreign', 'from', 'grant', 'group', 'having', 'in', 'into', 'is', 'join', 'limit', 'not', 'null', 'offset', 'on', 'or', 'order', 'primary', 'references', 'select', 'table', 'then', 'to', 'true', 'union', 'unique', 'user', 'using', 'when', 'where', 'with'])

export function qualified(schema: string, table: string): string {
  return `${quoteIdent(schema)}.${quoteIdent(table)}`
}

/** Rows as tab-separated text (with a header), NULL as empty, tabs and newlines escaped. */
export function toTsv(columns: string[], rows: (string | null)[][]): string {
  const cell = (v: string | null) => (v === null ? '' : v.replace(/\\/g, '\\\\').replace(/\t/g, '\\t').replace(/\r?\n/g, '\\n'))
  return [columns.map((c) => cell(c)).join('\t'), ...rows.map((r) => r.map(cell).join('\t'))].join('\n')
}

/** Right-aligned in the grid: a whole column of numbers. */
export function numericColumn(rows: (string | null)[][], col: number): boolean {
  let seen = 0
  for (const r of rows) {
    const v = r[col]
    if (v === null) continue
    if (!/^-?\d+(\.\d+)?(e[+-]?\d+)?$/i.test(v)) return false
    if (++seen >= 200) break
  }
  return seen > 0
}

/** Line and column (1-based) of a 1-based character position in `text` (SQL errors). */
export function positionToLineColumn(text: string, position: number): { line: number; column: number } {
  const before = text.slice(0, Math.max(0, position - 1))
  const lines = before.split('\n')
  return { line: lines.length, column: lines[lines.length - 1].length + 1 }
}

/** `user@host:port/db` for a source, from its fields (the URL secret is not read here). */
export function sourceLabel(s: { host: string; port: number | null; database: string; user: string; url: string }): string {
  if (!s.host && s.url) return `from secret ${s.url}`
  const host = s.host || 'localhost'
  return `${s.user ? `${s.user}@` : ''}${host}${s.port ? `:${s.port}` : ''}${s.database ? `/${s.database}` : ''}`
}

export function newConsoleId(): string {
  return `c${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`
}
