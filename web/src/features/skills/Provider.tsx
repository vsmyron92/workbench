// The skills slice's provider: the lists are read from disk, so they are refetched when
// something that decides them changed (Workbench installed or removed a skill, config.toml was
// saved, a project was added or moved). Like every provider it wraps the app, so it renders its
// children.

import type { ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useInvalidateOn } from '@/api/events'
import { sk } from './api'

const EVENTS = ['skills.changed', 'settings.changed', 'projects.changed']

export function SkillsProvider({ children }: { children?: ReactNode }) {
  const qc = useQueryClient()
  useInvalidateOn(qc, EVENTS, () => sk.all)
  return <>{children}</>
}
