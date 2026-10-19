// What the skills views share: opening a skill's panel, a project skill in the editor, copying.

import { openPanel, toast } from '@/shell/actions'
import { copyText } from '@/ui'
import { useSkillsUi } from './store'
import type { SkillInfo } from './types'

export const skillPanelId = (projectId: string, skillId: string) => `skill:${projectId}:${skillId}`

/** The panel of a skill found on disk (`s…`, `b…`) or of one of Workbench's own (`wb:<name>`). */
export function openSkill(projectId: string, skillId: string, title: string) {
  useSkillsUi.getState().select(skillId)
  openPanel({ kind: 'skill', id: skillPanelId(projectId, skillId), title, params: { projectId, skillId } })
}

/** A skill that lies inside the project, in the editor (the files slice's `editor` panel). */
export function openInEditor(projectId: string, projectFile: string) {
  const base = projectFile.split('/').pop() ?? projectFile
  openPanel({ kind: 'editor', id: `editor:${projectId}:${projectFile}`, title: base, params: { projectId, path: projectFile } })
}

export async function copy(text: string, what: string) {
  if (await copyText(text)) toast('success', `Copied ${what}`, { timeout: 1800 })
  else toast('error', 'Could not copy to the clipboard')
}

export function titleOf(s: Pick<SkillInfo, 'qualified'>): string {
  return s.qualified.length > 30 ? `${s.qualified.slice(0, 29)}…` : s.qualified
}
