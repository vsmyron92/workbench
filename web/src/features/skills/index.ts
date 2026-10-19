// Feature slice: skills. Owned by the skills slice: see docs/ARCHITECTURE.md ("Agent skills").
// Which skills the agent CLIs can load in a project, from where, and what they cost every session,
// and the optional skills Workbench offers to teach agents to use it. Read-only apart from installing
// and removing Workbench's own. The `skill` panel shows one skill; the Skills tool window lists them.

import { lazy } from 'react'
import { GraduationCap } from 'lucide-react'
import { showToolWindow } from '@/shell/actions'
import type { FeatureModule } from '@/shell/types'
import { SkillsProvider } from './Provider'
import './skills.css'

const SkillPanel = lazy(() => import('./SkillPanel').then((m) => ({ default: m.SkillPanel })))
const SkillsToolWindow = lazy(() => import('./SkillsToolWindow').then((m) => ({ default: m.SkillsToolWindow })))

const feature: FeatureModule = {
  id: 'skills',
  panels: {
    // One skill, found on disk or one of Workbench's own: `{projectId, skillId}` (`wb:<name>` for the latter).
    skill: { component: SkillPanel, icon: GraduationCap },
  },
  toolWindows: [{ id: 'skills', title: 'Skills', icon: GraduationCap, side: 'right', order: 70, component: SkillsToolWindow }],
  commands: (ctx) =>
    ctx.projectId
      ? [
          {
            id: 'skills.show',
            title: 'Show Agent Skills',
            group: 'Tool windows',
            keywords: ['skill', 'skills', 'agent', 'claude', 'codex', 'plugin', 'SKILL.md', 'slash command', 'context'],
            icon: GraduationCap,
            run: () => showToolWindow('skills', 'right'),
          },
        ]
      : [],
  providers: [SkillsProvider],
}

export default feature
