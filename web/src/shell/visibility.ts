// Which tool windows and phone tabs apply to the current project (their `when`).

import type { ProjectSummary } from '@/api/types'

/** The items whose `when` accepts `project` (items without one always apply). */
export function visibleFor<T extends { when?: (project: ProjectSummary | null) => boolean }>(items: T[], project: ProjectSummary | null): T[] {
  return items.filter((t) => !t.when || t.when(project))
}

/** The tab to show: the chosen one while it is visible, else the first visible one. */
export function activeTab<T extends { id: string }>(visible: T[], chosen: string): T | undefined {
  return visible.find((t) => t.id === chosen) ?? visible[0]
}
