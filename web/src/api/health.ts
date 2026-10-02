// GET /api/health: the server's version and OS, and what that OS leaves out
// (`unsupported`, feature → reason) or offers only as `experimental` (feature → note).
// Both are empty on Linux. Loaded once after sign-in; the UI hides what cannot work
// (dev containers on Windows) and says why where a feature is only limited.

import { useSyncExternalStore } from 'react'
import { api } from './client'

/** app.rs `health` (feature keys: util::os::support::Feature::key). */
export interface Health {
  ok: boolean
  service: string
  version: string
  startedAt: number
  os?: string
  unsupported?: Record<string, string>
  experimental?: Record<string, string>
}

/** Features whose support depends on the server's OS. */
export const FEATURES = {
  devcontainer: 'devcontainer',
  desktopNotifications: 'desktopNotifications',
  gdbAttach: 'gdbAttach',
  rustGdbPrettyPrinters: 'rustGdbPrettyPrinters',
  networkRoots: 'networkRoots',
  services: 'services',
  selfUpdate: 'selfUpdate',
} as const

export type Feature = (typeof FEATURES)[keyof typeof FEATURES]

let report: Health | null = null
const listeners = new Set<() => void>()

/** The loaded report; null until it arrives. */
export function getHealth(): Health | null {
  return report
}

/** Replace the report (`loadHealth`; tests). */
export function setHealth(h: Health | null) {
  report = h
  listeners.forEach((l) => l())
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** The report, re-rendering when it arrives. */
export function useHealth(): Health | null {
  return useSyncExternalStore(subscribe, getHealth, getHealth)
}

/** Fetch the health report once (until a load succeeds); until then everything counts as supported. */
export async function loadHealth(): Promise<void> {
  if (report) return
  try {
    setHealth(await api.get<Health>('/api/health'))
  } catch {
    /* an older or unreachable server: nothing is marked */
  }
}

/**
 * Ask again, whatever was loaded: the server may be another version after a restart
 * (`features/platform/update.ts`). Null when it does not answer.
 */
export async function refreshHealth(): Promise<Health | null> {
  try {
    const h = await api.get<Health>('/api/health')
    setHealth(h)
    return h
  } catch {
    return null
  }
}

/** Why `feature` does not work on the server's OS, or null. */
export function unsupportedReason(feature: Feature, health: Health | null = report): string | null {
  return sentence(health?.unsupported?.[feature])
}

/** What to know about `feature` being experimental on the server's OS, or null. */
export function experimentalNote(feature: Feature, health: Health | null = report): string | null {
  return sentence(health?.experimental?.[feature])
}

/** `unsupportedReason`, re-rendering when the report arrives. */
export function useUnsupported(feature: Feature): string | null {
  return unsupportedReason(feature, useHealth())
}

/** `experimentalNote`, re-rendering when the report arrives. */
export function useExperimental(feature: Feature): string | null {
  return experimentalNote(feature, useHealth())
}

/** `windows` → `Windows` (the server's `os`). */
export function osLabel(os: string | null | undefined): string | null {
  if (!os) return null
  return ({ linux: 'Linux', windows: 'Windows', macos: 'macOS' } as Record<string, string>)[os] ?? os
}

/**
 * Where the server's OS keeps config.toml unless WORKBENCH_CONFIG_DIR moves it (the server's
 * `dirs::config_dir()/workbench`), for setup hints; the Linux place until the report arrives.
 */
export function configFileHint(os: string | null | undefined = report?.os): string {
  if (os === 'windows') return '%APPDATA%\\workbench\\config.toml'
  if (os === 'macos') return '~/Library/Application Support/workbench/config.toml'
  return '~/.config/workbench/config.toml'
}

/** The server writes reasons like its error messages (lowercase first letter). */
function sentence(s: string | undefined): string | null {
  const t = s?.trim()
  return t ? t[0].toUpperCase() + t.slice(1) : null
}
