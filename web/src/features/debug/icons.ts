// Icons of the debug feature (lucide), in one place.

import { Box, Bug, Cpu, FileCode, Hammer, Package } from 'lucide-react'
import type { LaunchConfig } from './types'

export const ORIGIN_ICON: Record<LaunchConfig['origin'], typeof Bug> = {
  config: Bug,
  cargo: Package,
  cmake: Hammer,
  python: FileCode,
  go: Box,
}

/** A chip for a remote target (embedded), whatever its configuration came from. */
export function configIcon(c: Pick<LaunchConfig, 'origin' | 'remote'>): typeof Bug {
  return c.remote ? Cpu : ORIGIN_ICON[c.origin]
}
