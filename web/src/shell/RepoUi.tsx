// Repository pieces the git, GitLab and GitHub features share: a plain select for the phone's
// tabs, and what a GitLab or GitHub window shows while the project's active repository is
// not hosted there (it says so, and offers the project's repositories that are).

import type { ComponentType } from 'react'
import { scopeProject, setActiveRepo, type Forge } from '@/api/repos'
import { useActiveRepo } from '@/api/useRepos'
import { Button, EmptyState, Select } from '@/ui'

/** A plain select of the project's repositories for the phone's Git and CI tabs (nothing for a single repository). */
export function RepoSelect({ projectId }: { projectId: string | null }) {
  const { repo, repos, set } = useActiveRepo(projectId)
  if (!projectId || !repo || repos.length < 2) return null
  return (
    <Select className="git-repo-select" aria-label="Repository" value={repo.id} onChange={(e) => set(e.target.value)}>
      {repos.map((r) => (
        <option key={r.id} value={r.id}>
          {r.name}
        </option>
      ))}
    </Select>
  )
}

const LABELS: Record<Forge, string> = { gitlab: 'GitLab', github: 'GitHub' }

export function NotOnForge({ scope, forge, icon }: { scope: string; forge: Forge; icon?: ComponentType<{ size?: number; className?: string }> }) {
  const projectId = scopeProject(scope)
  const { repo, repos } = useActiveRepo(projectId)
  const label = LABELS[forge]
  const others = repos.filter((r) => r[forge] && r.id !== repo?.id)
  return (
    <EmptyState
      icon={icon}
      title={repos.length > 1 && repo ? `${repo.name} is not on ${label}` : `This project is not on ${label}`}
      action={
        others.length ? (
          <div className="wb-row" style={{ flexWrap: 'wrap', justifyContent: 'center', gap: 6 }}>
            {others.map((r) => (
              <Button key={r.id} size="small" onClick={() => setActiveRepo(projectId, r.id)}>
                Switch to {r.name}
              </Button>
            ))}
          </div>
        ) : undefined
      }
    >
      {others.length ? `Other repositories of this project are on ${label}.` : `No repository of this project has a ${label} remote.`}
    </EmptyState>
  )
}
