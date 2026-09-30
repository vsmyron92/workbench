// The desktop layout: top bar, tool-window stripes and sides, the center dock
// with a bottom tool-window area, and the status bar.

import { Suspense, useEffect, useRef, useState } from 'react'
import { ChevronDown, Command as CommandIcon, FolderGit2, FolderPlus, Minus, Plug, Settings, Unplug } from 'lucide-react'
import { isEventsConnected, onEventsConnection } from '@/api/events'
import { useProjects } from '@/api/queries'
import type { ProjectSummary } from '@/api/types'
import { useUi } from '@/state/store'
import { IconButton, Loading, showMenuAt, Splitter, MenuHost, type MenuEntry } from '@/ui'
import { addProjectInteractive, openSettings } from './actions'
import { CommandPalette, openPalette } from './CommandPalette'
import { SearchEverywhere } from './SearchEverywhere'
import { Dock } from './Dock'
import { NoProjectsBanner } from './NoProjects'
import { Dialogs, Toasts } from './Overlays'
import { statusbarWidgets, toolWindows, topbarWidgets } from './registry'
import type { Side, ToolWindowDef } from './types'
import { visibleFor } from './visibility'

function visibleToolWindows(side: Side, project: ProjectSummary | null): ToolWindowDef[] {
  return visibleFor(toolWindows, project).filter((t) => t.side === side)
}

function Stripe({ sides, project }: { sides: Side[]; project: ProjectSummary | null }) {
  const state = useUi((s) => s.sides)
  const toggle = useUi((s) => s.toggleToolWindow)
  return (
    <div className="wb-stripe">
      {sides.map((side, i) => (
        <div key={side} className="wb-stripe-group" style={i > 0 ? { marginTop: 'auto' } : undefined}>
          {visibleToolWindows(side, project).map((t) => (
            <div key={t.id} className="wb-stripe-item">
              <IconButton icon={t.icon} label={t.title} active={state[side].active === t.id} onClick={() => toggle(side, t.id)} />
              {t.badge && (
                <span className="wb-stripe-badge">
                  <t.badge projectId={project?.id ?? null} />
                </span>
              )}
            </div>
          ))}
        </div>
      ))}
    </div>
  )
}

function ToolWindowArea({ side, project }: { side: Side; project: ProjectSummary | null }) {
  const sideState = useUi((s) => s.sides[side])
  const hide = useUi((s) => s.hideSide)
  const def = visibleToolWindows(side, project).find((t) => t.id === sideState.active)
  if (!def) return null
  const C = def.component
  return (
    <div className={`wb-toolwindow ${side}`}>
      <div className="wb-toolwindow-header">
        <span className="title">{def.title}</span>
        <span style={{ flex: 1 }} />
        <IconButton icon={Minus} size="small" label="Hide" onClick={() => hide(side)} />
      </div>
      <div className="wb-toolwindow-body">
        {/* Features may register tool windows with React.lazy. */}
        <Suspense fallback={<Loading />}>
          <C projectId={project?.id ?? null} />
        </Suspense>
      </div>
    </div>
  )
}

function SideResizer({ side }: { side: Side }) {
  const size = useUi((s) => s.sides[side].size)
  const setSize = useUi((s) => s.setSideSize)
  const start = useRef(size)
  const sign = side === 'left' ? 1 : -1
  const [min, max] = side === 'bottom' ? [100, window.innerHeight - 160] : [180, Math.max(260, window.innerWidth * 0.6)]
  return (
    <Splitter
      direction={side === 'bottom' ? 'h' : 'v'}
      onResizeStart={() => (start.current = useUi.getState().sides[side].size)}
      onResize={(d) => setSize(side, Math.max(min, Math.min(max, start.current + sign * d)))}
    />
  )
}

function ProjectSwitcher({ projects, current }: { projects: ProjectSummary[]; current: ProjectSummary | null }) {
  const setProject = useUi((s) => s.setProject)
  const open = (el: HTMLElement) => {
    const items: MenuEntry[] = projects.map((p) => ({
      label: p.name + (p.branch ? `  ·  ${p.branch}` : ''),
      icon: FolderGit2,
      run: () => setProject(p.id),
    }))
    items.push('separator', { label: 'Add project…', icon: FolderPlus, run: () => void addProjectInteractive() })
    showMenuAt(el, items)
  }
  return (
    <button className="wb-project-switcher" onClick={(e) => open(e.currentTarget)} title={current?.root}>
      <span className="wb-project-avatar">{(current?.name ?? '?').slice(0, 2).toUpperCase()}</span>
      <span className="wb-ellipsis">{current?.name ?? 'No project'}</span>
      <ChevronDown size={14} className="wb-muted" />
    </button>
  )
}

function ConnectionIndicator() {
  const [up, setUp] = useState(isEventsConnected())
  useEffect(() => onEventsConnection(setUp), [])
  return up ? (
    <span className="wb-status-item" title="Connected to Workbench">
      <Plug size={13} />
    </span>
  ) : (
    <span className="wb-status-item wb-warning" title="Reconnecting…">
      <Unplug size={13} /> Reconnecting…
    </span>
  )
}

export function DesktopShell() {
  const { data: projects, refetch } = useProjects()
  const projectId = useUi((s) => s.projectId)
  const setProject = useUi((s) => s.setProject)
  const sides = useUi((s) => s.sides)
  const project = projects?.find((p) => p.id === projectId) ?? null

  // A project id the list does not have is stale, or just added (Add project…): fetch the list
  // again before falling back to the first project.
  useEffect(() => {
    if (!projects || !projects.length || projects.some((p) => p.id === projectId)) return
    let cancelled = false
    void refetch().then((r) => {
      const list = r.data
      if (!cancelled && list?.length && !list.some((p) => p.id === projectId)) setProject(list[0].id)
    })
    return () => {
      cancelled = true
    }
  }, [projects, projectId, setProject, refetch])

  const pid = project?.id ?? null
  const hasLeft = !!visibleToolWindows('left', project).find((t) => t.id === sides.left.active)
  const hasRight = !!visibleToolWindows('right', project).find((t) => t.id === sides.right.active)
  const hasBottom = !!visibleToolWindows('bottom', project).find((t) => t.id === sides.bottom.active)

  return (
    <div className="wb-shell">
      <header className="wb-topbar">
        <IconButton icon={Settings} label="Settings (Ctrl+,)" onClick={() => openSettings()} />
        <img src="/favicon.svg" alt="" width={20} height={20} className="wb-logo" />
        <ProjectSwitcher projects={projects ?? []} current={project} />
        {topbarWidgets.map((W, i) => (
          <W key={i} projectId={pid} />
        ))}
        <span style={{ flex: 1 }} />
        <button className="wb-search-button" onClick={() => openPalette()}>
          <CommandIcon size={13} /> Commands <kbd className="wb-kbd">Ctrl+K</kbd>
        </button>
      </header>
      <div className="wb-main">
        <Stripe sides={['left', 'bottom']} project={project} />
        {hasLeft && (
          <>
            <div style={{ width: sides.left.size }} className="wb-side">
              <ToolWindowArea side="left" project={project} />
            </div>
            <SideResizer side="left" />
          </>
        )}
        <div className="wb-center">
          <NoProjectsBanner />
          <div className="wb-dock-area">
            <Dock />
          </div>
          {hasBottom && (
            <>
              <SideResizer side="bottom" />
              <div style={{ height: sides.bottom.size }} className="wb-bottom">
                <ToolWindowArea side="bottom" project={project} />
              </div>
            </>
          )}
        </div>
        {hasRight && (
          <>
            <SideResizer side="right" />
            <div style={{ width: sides.right.size }} className="wb-side">
              <ToolWindowArea side="right" project={project} />
            </div>
          </>
        )}
        <Stripe sides={['right']} project={project} />
      </div>
      <footer className="wb-statusbar">
        {statusbarWidgets.map((W, i) => (
          <W key={i} projectId={pid} />
        ))}
        <span style={{ flex: 1 }} />
        <ConnectionIndicator />
      </footer>
      <CommandPalette />
      <SearchEverywhere />
      <Toasts />
      <Dialogs />
      <MenuHost />
    </div>
  )
}
