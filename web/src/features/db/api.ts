// REST of the db slice (server/src/db): data sources, their catalog, SQL consoles.

import { useQuery } from '@tanstack/react-query'
import { api } from '@/api/client'

const enc = encodeURIComponent
const base = (pid: string) => `/api/projects/${enc(pid)}/db`

export interface DbSource {
  name: string
  kind: string
  host: string
  port: number | null
  database: string
  user: string
  /** Secret names, never values. */
  password: string
  url: string
  sslmode: string
  readOnly: boolean
  /** Where the entry lives: the machine overlay (editable here) or the repository. */
  origin: 'overlay' | 'repository'
}

export interface DbSources {
  sources: DbSource[]
  secretNames: string[]
  overlayPath: string
}

export interface Relation {
  name: string
  kind: 'table' | 'view' | 'matview' | 'partitioned' | 'foreign'
  rows: number | null
  comment: string | null
}

export interface Catalog {
  database: string
  schemas: { name: string; relations: Relation[] }[]
  truncated: boolean
}

export interface TableInfo {
  columns: { name: string; dataType: string; nullable: boolean; default: string | null; primaryKey: boolean; comment: string | null }[]
  indexes: { name: string; definition: string }[]
  foreignKeys: { name: string; definition: string }[]
}

export interface ResultSet {
  columns: string[]
  rows: (string | null)[][]
  truncated: boolean
  rowsAffected: number | null
}

export interface SqlError {
  message: string
  code: string | null
  detail: string | null
  hint: string | null
  /** 1-based character position in the SQL sent. */
  position: number | null
  severity: string | null
}

export interface QueryOutcome {
  results: ResultSet[]
  error: SqlError | null
  notices: string[]
  ms: number
  stopped: boolean
}

export interface TestResult {
  version: string
  ssl: boolean
  user: string
  ms: number
  display: string
  sslmode: string
}

export const dbKeys = {
  sources: (pid: string) => ['db', pid, 'sources'] as const,
  catalog: (pid: string, source: string) => ['db', pid, 'catalog', source] as const,
  table: (pid: string, source: string, schema: string, table: string) => ['db', pid, 'table', source, schema, table] as const,
}

export function useDbSources(pid: string | null) {
  return useQuery({
    queryKey: dbKeys.sources(pid ?? ''),
    queryFn: ({ signal }) => api.get<DbSources>(base(pid!), undefined, signal),
    enabled: !!pid,
  })
}

export function useCatalog(pid: string, source: string, enabled: boolean) {
  return useQuery({
    queryKey: dbKeys.catalog(pid, source),
    queryFn: ({ signal }) => api.get<Catalog>(`${base(pid)}/${enc(source)}/schema`, undefined, signal),
    enabled,
    retry: false,
    staleTime: 5 * 60_000,
  })
}

export function useTableInfo(pid: string, source: string, schema: string, table: string, enabled: boolean) {
  return useQuery({
    queryKey: dbKeys.table(pid, source, schema, table),
    queryFn: ({ signal }) => api.get<TableInfo>(`${base(pid)}/${enc(source)}/table`, { schema, table }, signal),
    enabled,
    retry: false,
    staleTime: 5 * 60_000,
  })
}

export const dbApi = {
  test: (pid: string, source: string) => api.post<TestResult>(`${base(pid)}/${enc(source)}/test`),
  query: (pid: string, source: string, sql: string, consoleId: string, maxRows: number) =>
    api.post<QueryOutcome>(`${base(pid)}/${enc(source)}/query`, { sql, console: consoleId, maxRows }),
  cancel: (pid: string, source: string, consoleId: string) => api.post<{ running: boolean }>(`${base(pid)}/${enc(source)}/consoles/${enc(consoleId)}/cancel`),
  close: (pid: string, source: string, consoleId: string) => api.del(`${base(pid)}/${enc(source)}/consoles/${enc(consoleId)}`),
  putSource: (pid: string, source: Omit<DbSource, 'origin'>, previousName?: string) =>
    api.put(`${base(pid)}/_sources/${enc(source.name)}`, {
      source: { ...source, read_only: source.readOnly, port: source.port || null },
      previousName,
    }),
  deleteSource: (pid: string, name: string) => api.del(`${base(pid)}/_sources/${enc(name)}`),
}
