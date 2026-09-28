// Icons of the debug feature (lucide), in one place.

import { Box, Bug, FileCode, Hammer, Package } from 'lucide-react'
import type { LaunchConfig } from './types'

export const ORIGIN_ICON: Record<LaunchConfig['origin'], typeof Bug> = {
  config: Bug,
  cargo: Package,
  cmake: Hammer,
  python: FileCode,
  go: Box,
}
