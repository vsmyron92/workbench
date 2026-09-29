// Composes every feature module. Adding a feature = one import + one array entry.

import agents from '@/features/agents'
import apps from '@/features/apps'
import atlassian from '@/features/atlassian'
import db from '@/features/db'
import debug from '@/features/debug'
import devcontainer from '@/features/devcontainer'
import files from '@/features/files'
import git from '@/features/git'
import github from '@/features/github'
import gitlab from '@/features/gitlab'
import help from '@/features/help'
import lsp from '@/features/lsp'
import platform from '@/features/platform'
import workspace from '@/features/workspace'
import { toolWindowSides } from './actions'
import type { Command, CommandContext, FeatureModule, MobileTabDef, PanelDef, SearchProvider, ToolWindowDef } from './types'

export const features: FeatureModule[] = [agents, files, lsp, debug, git, gitlab, github, atlassian, apps, devcontainer, db, workspace, help, platform]

export const panelDefs: Record<string, PanelDef> = Object.assign({}, ...features.map((f) => f.panels ?? {}))

export const toolWindows: ToolWindowDef[] = features.flatMap((f) => f.toolWindows ?? []).sort((a, b) => a.order - b.order)
toolWindows.forEach((t) => toolWindowSides.set(t.id, t.side))

export const topbarWidgets = features.flatMap((f) => f.topbar ?? [])
export const statusbarWidgets = features.flatMap((f) => f.statusbar ?? [])
export const mobileTabs: MobileTabDef[] = features.flatMap((f) => f.mobileTabs ?? []).sort((a, b) => a.order - b.order)
export const providers = features.flatMap((f) => f.providers ?? [])
export const searchProviders: SearchProvider[] = features.flatMap((f) => f.searchProviders ?? []).sort((a, b) => a.order - b.order)

export function featureCommands(ctx: CommandContext): Command[] {
  return features.flatMap((f) => {
    try {
      return (f.commands?.(ctx) ?? []).filter((c) => !c.when || c.when(ctx))
    } catch (e) {
      console.error(`commands of ${f.id} failed`, e)
      return []
    }
  })
}
