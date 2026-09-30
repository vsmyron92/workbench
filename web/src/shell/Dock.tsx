// The center area: IDE-style tabs and splits (dockview). Panels are the kinds
// features register; the layout is saved per browser and restored on load.

import { Component, Suspense, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import {
  DockviewReact,
  themeAbyss,
  type DockviewApi,
  type DockviewReadyEvent,
  type DockviewTheme,
  type IDockviewPanelProps,
} from 'dockview-react'
import 'dockview-react/dist/styles/dockview.css'
import { useUi } from '@/state/store'
import { Loading, ErrorBox } from '@/ui'
import { noteActiveGroup, setDockApi } from './actions'
import { panelDefs } from './registry'
import { Welcome } from './Welcome'
import type { PanelProps } from './types'

// One layout (tabs and splits) per project, so switching projects switches the tabs.
// `wb.layout.v1` is the single layout older versions saved: the first project restores it.
const LEGACY_LAYOUT_KEY = 'wb.layout.v1'
const layoutKey = (pid: string | null) => (pid ? `wb.layout.v2.${pid}` : LEGACY_LAYOUT_KEY)

const theme: DockviewTheme = {
  ...themeAbyss,
  name: 'workbench',
  className: 'dockview-theme-abyss dockview-theme-workbench',
}

class PanelBoundary extends Component<{ children: ReactNode }, { error: unknown }> {
  state = { error: null as unknown }
  static getDerivedStateFromError(error: unknown) {
    return { error }
  }
  render() {
    if (this.state.error) return <ErrorBox error={this.state.error} onRetry={() => this.setState({ error: null })} />
    return this.props.children
  }
}

function makeHost(kind: string) {
  return function PanelHost(props: IDockviewPanelProps<Record<string, unknown>>) {
    const def = panelDefs[kind]
    const [visible, setVisible] = useState(props.api.isVisible)
    const [active, setActive] = useState(props.api.isActive)
    useEffect(() => {
      const a = props.api.onDidVisibilityChange((e) => setVisible(e.isVisible))
      const b = props.api.onDidActiveChange((e) => setActive(e.isActive))
      return () => {
        a.dispose()
        b.dispose()
      }
    }, [props.api])
    if (!def) {
      return (
        <div className="wb-empty">
          <div className="title">This panel is no longer available</div>
          <div className="wb-small">Unknown panel kind “{kind}”.</div>
        </div>
      )
    }
    const Comp = def.component
    const panelProps: PanelProps = {
      id: props.api.id,
      params: props.params ?? {},
      setParams: (p) => props.api.updateParameters(p),
      setTitle: (t) => props.api.setTitle(t),
      close: () => props.api.close(),
      visible,
      active,
    }
    return (
      <div className="wb-panel-host">
        <PanelBoundary>
          <Suspense fallback={<Loading />}>
            <Comp {...panelProps} />
          </Suspense>
        </PanelBoundary>
      </div>
    )
  }
}

// Any kind name resolves to a host, so a saved layout that mentions a removed
// feature still restores (and shows a placeholder) instead of failing wholesale.
const hosts = new Map<string, ReturnType<typeof makeHost>>()
const components = new Proxy({} as Record<string, ReturnType<typeof makeHost>>, {
  get(_t, kind: string | symbol) {
    if (typeof kind !== 'string') return undefined
    let h = hosts.get(kind)
    if (!h) hosts.set(kind, (h = makeHost(kind)))
    return h
  },
  has() {
    return true
  },
})

function Watermark() {
  return <Welcome />
}

export function Dock() {
  const projectId = useUi((s) => s.projectId)
  const [dock, setDock] = useState<DockviewApi | null>(null)
  const shown = useRef<string | null | undefined>(undefined)
  const saveTimer = useRef<number | undefined>(undefined)

  const onReady = useMemo(
    () => (e: DockviewReadyEvent) => {
      const api = e.api
      // Keep-alive panels (terminals, editors) render even while their tab is hidden.
      api.onDidAddPanel((p) => {
        if (panelDefs[p.view.contentComponent]?.keepAlive) p.api.setRenderer('always')
      })
      api.onDidLayoutChange(() => {
        window.clearTimeout(saveTimer.current)
        const key = layoutKey(shown.current ?? null)
        if (shown.current === undefined) return
        saveTimer.current = window.setTimeout(() => saveLayout(api, key), 400)
      })
      api.onDidActiveGroupChange((g) => noteActiveGroup(g))
      // dockview 8.3: removing the active group disposes it and *then* deactivates it,
      // which re-creates the group's watermark (our Welcome) as a React portal nobody
      // disposes: ~75 DOM nodes and ~150 listeners per closed document column. The
      // deactivation runs synchronously after this event, so clean up right after.
      api.onDidRemoveGroup((g) =>
        queueMicrotask(() => {
          const model = g.model as unknown as { watermark?: { element: HTMLElement; dispose?: () => void } }
          const orphan = model.watermark
          if (!orphan) return
          model.watermark = undefined
          orphan.element.remove()
          orphan.dispose?.()
        }),
      )
      setDockApi(api)
      setDock(api)
    },
    [],
  )

  // Save the layout of the project being left and restore the one being entered.
  useEffect(() => {
    if (!dock || shown.current === projectId) return
    window.clearTimeout(saveTimer.current)
    const prev = shown.current
    if (prev) saveLayout(dock, layoutKey(prev))
    // Not yet restoring: layout events fired by clear() and fromJSON() must not save.
    shown.current = undefined
    if (prev !== undefined) dock.clear()
    let saved = localStorage.getItem(layoutKey(projectId))
    if (!saved && !prev && !localStorage.getItem(MIGRATED_KEY)) saved = localStorage.getItem(LEGACY_LAYOUT_KEY)
    if (saved) {
      try {
        dock.fromJSON(JSON.parse(saved))
      } catch (err) {
        console.warn('layout restore failed', err)
        localStorage.removeItem(layoutKey(projectId))
      }
    }
    if (!prev && projectId) {
      try {
        localStorage.setItem(MIGRATED_KEY, '1')
      } catch {
        /* quota */
      }
    }
    for (const p of dock.panels) {
      if (panelDefs[p.view.contentComponent]?.keepAlive) p.api.setRenderer('always')
    }
    shown.current = projectId
  }, [dock, projectId])

  useEffect(
    () => () => {
      window.clearTimeout(saveTimer.current)
      setDockApi(null)
    },
    [],
  )
  return (
    <DockviewReact
      className="wb-dock"
      components={components}
      onReady={onReady}
      theme={theme}
      watermarkComponent={Watermark}
      defaultRenderer="onlyWhenVisible"
    />
  )
}

const MIGRATED_KEY = 'wb.layout.v2.migrated'

function saveLayout(api: DockviewApi, key: string) {
  try {
    localStorage.setItem(key, JSON.stringify(api.toJSON()))
  } catch {
    /* quota */
  }
}
