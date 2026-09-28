// Search Everywhere (double Shift, as in CLion): one popup over the tabs features
// contribute (Files, Symbols, Text…) plus Actions (the palette's commands). "All"
// shows the best few of each; Tab / Shift+Tab switch tabs, Enter opens, Shift+Enter
// opens to the side. Mounted once by the desktop shell.

import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Command as Cmdk } from 'cmdk'
import { PanelLeft } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { Kbd, Spinner } from '@/ui'
import { formatShortcut, paletteCommands, useCommandContext } from './CommandPalette'
import { doubleShiftDetector, rankCommands } from './paletteSearch'
import { searchProviders } from './registry'
import { SEARCH_ALL, useSearchEverywhere } from './searchEverywhereStore'
import type { SearchItem, SearchProvider } from './types'

const ALL = SEARCH_ALL
const DEBOUNCE_MS = 120
const ALL_DEFAULT = 5
const TAB_MAX = 100

export function SearchEverywhere() {
  const open = useSearchEverywhere((s) => s.open)
  useEffect(() => {
    const d = doubleShiftDetector(() => {
      const st = useSearchEverywhere.getState()
      if (st.open) st.hide()
      else st.show()
    })
    // Capture phase, never consuming anything: Shift alone types nothing, so this
    // works in terminals and editors too.
    const down = (e: KeyboardEvent) => d.keydown(e, performance.now())
    const up = (e: KeyboardEvent) => d.keyup(e, performance.now())
    const blur = () => d.reset()
    window.addEventListener('keydown', down, true)
    window.addEventListener('keyup', up, true)
    window.addEventListener('blur', blur)
    return () => {
      window.removeEventListener('keydown', down, true)
      window.removeEventListener('keyup', up, true)
      window.removeEventListener('blur', blur)
    }
  }, [])
  return open ? <SearchDialog /> : null
}

interface Found {
  loading: boolean
  items: SearchItem[]
  error?: string
}

function SearchDialog() {
  const { tab, show, hide } = useSearchEverywhere()
  const current = useCommandContext()
  // One context object per project, so the searches below do not rerun on every render.
  const ctx = useMemo(() => current, [current.projectId, current.project]) // eslint-disable-line react-hooks/exhaustive-deps
  const { data: projects } = useProjects()
  const [query, setQuery] = useState('')
  const [debounced, setDebounced] = useState('')
  const [found, setFound] = useState<Record<string, Found>>({})
  const [selected, setSelected] = useState('')
  const input = useRef<HTMLInputElement>(null)

  // Actions are the palette's commands, ranked like the palette ranks them.
  const providers = useMemo(() => {
    const commands = paletteCommands(ctx, projects ?? [])
    const actions: SearchProvider = {
      id: 'actions',
      title: 'Actions',
      order: 30,
      search: async (q) =>
        rankCommands(commands, q)
          .slice(0, TAB_MAX)
          .map((c) => {
            const Icon = c.icon ?? PanelLeft
            return {
              key: c.id,
              title: c.title,
              detail: c.group,
              icon: <Icon size={15} />,
              hint: c.shortcut ? <Kbd>{formatShortcut(c.shortcut)}</Kbd> : undefined,
              run: () => void c.run(ctx),
            }
          }),
    }
    return [...searchProviders, actions].filter((p) => !p.when || p.when(ctx)).sort((a, b) => a.order - b.order)
  }, [ctx, projects])

  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(query), DEBOUNCE_MS)
    return () => window.clearTimeout(t)
  }, [query])

  // Ask the providers this tab shows.
  useEffect(() => {
    const q = debounced.trim()
    const asked = providers.filter((p) => (tab === ALL ? (p.inAll ?? ALL_DEFAULT) > 0 : p.id === tab))
    const ctrl = new AbortController()
    setFound((prev) => {
      const next: Record<string, Found> = {}
      for (const p of asked) next[p.id] = q.length >= (p.minQuery ?? 1) ? { loading: true, items: prev[p.id]?.items ?? [] } : { loading: false, items: [] }
      return next
    })
    for (const p of asked) {
      if (q.length < (p.minQuery ?? 1)) continue
      p.search(q, ctx, ctrl.signal).then(
        (items) => !ctrl.signal.aborted && setFound((f) => ({ ...f, [p.id]: { loading: false, items } })),
        (e: unknown) =>
          !ctrl.signal.aborted &&
          setFound((f) => ({ ...f, [p.id]: { loading: false, items: [], error: e instanceof Error ? e.message : String(e) } })),
      )
    }
    return () => ctrl.abort()
  }, [debounced, tab, providers, ctx])

  const sections = useMemo(() => {
    const shown = tab === ALL ? providers : providers.filter((p) => p.id === tab)
    return shown
      .map((p) => {
        const f = found[p.id]
        const items = f?.items ?? []
        const limit = tab === ALL ? (p.inAll ?? ALL_DEFAULT) : TAB_MAX
        return { p, items: items.slice(0, limit), more: tab === ALL && items.length > limit, f }
      })
      .filter((s) => s.items.length || (tab !== ALL && s.f))
  }, [providers, found, tab])

  const values = useMemo(
    () => sections.flatMap((s) => [...s.items.map((it) => `${s.p.id}:${it.key}`), ...(s.more ? [`more:${s.p.id}`] : [])]),
    [sections],
  )
  // The first row stays selected while results arrive (providers answer at different
  // times), until the user moves the selection; then keep theirs while it is shown.
  const picked = useRef(false)
  useEffect(() => {
    picked.current = false
  }, [debounced, tab])
  useEffect(() => {
    if (!picked.current || !values.includes(selected)) setSelected(values[0] ?? '')
  }, [values, selected])

  const itemFor = (value: string) => {
    const [pid, ...rest] = value.split(':')
    const key = rest.join(':')
    return found[pid]?.items.find((it) => it.key === key) ?? null
  }
  const run = (it: SearchItem, side: boolean) => {
    hide()
    it.run({ side })
  }

  const tabs = [{ id: ALL, title: 'All' }, ...providers.map((p) => ({ id: p.id, title: p.title }))]
  const loading = Object.values(found).some((f) => f.loading)
  const hints = providers
    .filter((p) => tab === ALL || p.id === tab)
    .map((p) => p.hint?.(ctx))
    .filter((h): h is string => !!h)
  const errors = sections.filter((s) => s.f?.error).map((s) => `${s.p.title}: ${s.f!.error}`)
  const q = debounced.trim()

  return (
    <Cmdk.Dialog
      open
      onOpenChange={(o) => !o && hide()}
      label="Search everywhere"
      className="wb-palette wb-se"
      overlayClassName="wb-palette-overlay"
      shouldFilter={false}
      loop
      value={selected}
      onValueChange={(v) => {
        picked.current = true
        setSelected(v)
      }}
    >
      <div className="wb-se-tabs" role="tablist">
        {tabs.map((t) => (
          <button
            key={t.id}
            role="tab"
            type="button"
            // Tab / Shift+Tab in the input switch tabs; the dialog's focus trap
            // would otherwise move focus to these buttons.
            tabIndex={-1}
            aria-selected={t.id === tab}
            className={t.id === tab ? 'active' : ''}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              show(t.id)
              input.current?.focus()
            }}
          >
            {t.title}
          </button>
        ))}
      </div>
      <Cmdk.Input
        ref={input}
        value={query}
        onValueChange={setQuery}
        placeholder={tab === ALL ? 'Search files, symbols, actions and text' : `Search ${tabs.find((t) => t.id === tab)?.title.toLowerCase()}`}
        autoFocus
        onKeyDown={(e) => {
          if (e.key === 'Tab') {
            e.preventDefault()
            const i = tabs.findIndex((t) => t.id === tab)
            show(tabs[(i + (e.shiftKey ? tabs.length - 1 : 1)) % tabs.length].id)
          } else if (e.key === 'Enter' && e.shiftKey) {
            const it = itemFor(selected)
            if (it) {
              e.preventDefault()
              run(it, true)
            }
          }
        }}
      />
      <Cmdk.List>
        {sections.map((s) => (
          <Cmdk.Group key={s.p.id} heading={tab === ALL ? s.p.title : undefined}>
            {s.items.map((it) => (
              <Cmdk.Item key={it.key} value={`${s.p.id}:${it.key}`} onSelect={() => run(it, false)}>
                <span className="wb-se-icon">{it.icon}</span>
                <span className="wb-se-title">
                  <Highlighted text={it.title} positions={it.highlight} />
                </span>
                {it.detail && <span className="wb-se-detail wb-ellipsis">{it.detail}</span>}
                {it.hint && <span className="wb-se-hint">{it.hint}</span>}
              </Cmdk.Item>
            ))}
            {s.more && (
              <Cmdk.Item value={`more:${s.p.id}`} onSelect={() => show(s.p.id)} className="wb-se-more">
                <span className="wb-se-icon" />
                <span className="wb-se-title">More {s.p.title.toLowerCase()}…</span>
                <span className="wb-se-hint">
                  <Kbd>Tab</Kbd>
                </span>
              </Cmdk.Item>
            )}
          </Cmdk.Group>
        ))}
        {!loading && !sections.length && (q || tab !== ALL) && <div className="wb-se-empty">{q ? 'Nothing found.' : 'Type to search.'}</div>}
      </Cmdk.List>
      {(hints.length > 0 || errors.length > 0) && (
        <div className="wb-se-notes">
          {errors.map((e) => (
            <div key={e} className="wb-danger">
              {e}
            </div>
          ))}
          {hints.map((h) => (
            <div key={h}>{h}</div>
          ))}
        </div>
      )}
      <div className="wb-se-footer">
        {loading && <Spinner size={10} />}
        <span className="wb-grow" />
        <Kbd>Tab</Kbd> next tab <Kbd>Enter</Kbd> open <Kbd>Shift+Enter</Kbd> to the side
      </div>
    </Cmdk.Dialog>
  )
}

function Highlighted({ text, positions }: { text: string; positions?: number[] }): ReactNode {
  if (!positions?.length) return text
  const hits = new Set(positions)
  const out: ReactNode[] = []
  let run = ''
  let runHit = false
  for (let i = 0; i < text.length; i++) {
    const hit = hits.has(i)
    if (hit !== runHit && run) {
      out.push(runHit ? <b key={i}>{run}</b> : run)
      run = ''
    }
    runHit = hit
    run += text[i]
  }
  if (run) out.push(runHit ? <b key="end">{run}</b> : run)
  return out
}
