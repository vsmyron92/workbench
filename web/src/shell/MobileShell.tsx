// Phone layout (remote control from a phone): one full-screen view with a bottom
// tab bar built from the features' `mobileTabs`. No dockview, no Monaco by default.

import { Suspense, useEffect, useMemo, useState } from 'react'
import { ChevronDown, FolderGit2, FolderPlus } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { useUi } from '@/state/store'
import { EmptyState, Loading, MenuHost, showMenuAt, type MenuEntry } from '@/ui'
import { addProjectInteractive, setMobileRouter } from './actions'
import { NoProjectsBanner } from './NoProjects'
import { Dialogs, Toasts } from './Overlays'
import { mobileTabs } from './registry'
import { activeTab, visibleFor } from './visibility'

function savedTab(): string {
  try {
    return sessionStorage.getItem('wb.mobile.tab') ?? ''
  } catch {
    return ''
  }
}

export function MobileShell() {
  const { data: projects } = useProjects()
  const projectId = useUi((s) => s.projectId)
  const setProject = useUi((s) => s.setProject)
  const [tab, setTab] = useState(() => savedTab() || mobileTabs[0]?.id || '')
  const project = projects?.find((p) => p.id === projectId) ?? null
  // The tabs that apply to this project (a GitLab project has no GitHub tab); a saved
  // tab that does not apply shows the first one until a project it applies to is chosen.
  const tabs = useMemo(() => visibleFor(mobileTabs, project), [project])

  useEffect(() => {
    if (projects && projects.length && !projects.some((p) => p.id === projectId)) setProject(projects[0].id)
  }, [projects, projectId, setProject])
  useEffect(() => {
    try {
      sessionStorage.setItem('wb.mobile.tab', tab)
    } catch {
      /* private mode */
    }
  }, [tab])
  // "Open" on a phone (ask agent, attention toasts, ui.open): the tab that can show it.
  useEffect(() => {
    setMobileRouter((panel) => {
      const t = tabs.find((m) => m.openPanel?.(panel))
      if (t) setTab(t.id)
      return !!t
    })
    return () => setMobileRouter(null)
  }, [tabs])

  const active = activeTab(tabs, tab)
  const C = active?.component
  return (
    <div className="wb-mobile">
      <header className="wb-mobile-top">
        <button
          className="wb-project-switcher"
          onClick={(e) =>
            showMenuAt(e.currentTarget, [
              ...(projects ?? []).map<MenuEntry>((p) => ({ label: p.name, icon: FolderGit2, run: () => setProject(p.id) })),
              'separator',
              { label: 'Add project…', icon: FolderPlus, run: () => void addProjectInteractive() },
            ])
          }
        >
          <span className="wb-ellipsis">{project?.name ?? 'No project'}</span>
          <ChevronDown size={14} />
        </button>
      </header>
      <main className="wb-mobile-main">
        <NoProjectsBanner phone />
        {C ? (
          <Suspense fallback={<Loading />}>
            <C projectId={project?.id ?? null} />
          </Suspense>
        ) : (
          <EmptyState title="Nothing to show on a phone yet" />
        )}
      </main>
      <nav className="wb-mobile-tabs">
        {tabs.map((t) => (
          <button key={t.id} className={t.id === active?.id ? 'active' : ''} onClick={() => setTab(t.id)}>
            <span className="icon">
              <t.icon size={20} />
              {t.badge && (
                <span className="wb-stripe-badge">
                  <t.badge projectId={project?.id ?? null} />
                </span>
              )}
            </span>
            <span className="label">{t.title}</span>
          </button>
        ))}
      </nav>
      <Toasts />
      <Dialogs />
      <MenuHost />
    </div>
  )
}
