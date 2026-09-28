// First run with no projects: say where projects come from and how to add one,
// above whatever panel is open (both shells).

import { useState } from 'react'
import { FolderPlus, RefreshCw, Settings2 } from 'lucide-react'
import { api } from '@/api/client'
import { useProjects } from '@/api/queries'
import { Button } from '@/ui'
import { addProjectInteractive, openSettings, toast, toastError } from './actions'

export function NoProjectsBanner({ phone = false }: { phone?: boolean }) {
  const { data: projects } = useProjects()
  const [busy, setBusy] = useState(false)
  if (!projects || projects.length) return null
  const reload = async () => {
    setBusy(true)
    try {
      const r = await api.post<{ count: number }>('/api/projects/reload')
      toast(r.count ? 'success' : 'info', r.count ? `Found ${r.count} project${r.count === 1 ? '' : 's'}` : 'Still no git repositories under the project roots')
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="wb-noprojects" role="status">
      <FolderPlus size={18} className="wb-noprojects-icon" />
      <div className="wb-grow">
        <div className="wb-noprojects-title">No projects yet</div>
        <div className="wb-small wb-muted">
          Workbench lists the git repositories directly under your project roots (<span className="mono">~/workspace</span> by default) and any
          directory you add. Clone one into a root and reload, or add a directory.
        </div>
      </div>
      <div className="wb-noprojects-actions">
        <Button variant="primary" size="small" icon={FolderPlus} onClick={() => void addProjectInteractive()}>
          Add project…
        </Button>
        <Button size="small" icon={RefreshCw} loading={busy} onClick={() => void reload()}>
          Reload
        </Button>
        {!phone && (
          <Button size="small" icon={Settings2} onClick={() => openSettings('projects')}>
            Project roots
          </Button>
        )}
      </div>
    </div>
  )
}
