// What is loaded with the app. The avatar's drawing and speech code is a lazy chunk: the
// provider, the status-bar widget, the palette commands and the phone's More tab are loaded
// with the app and must not pull it in. This follows the static imports from the app's
// entry through the sources (a lazy `import()` and a type-only import load nothing) and
// checks that none of them reaches that code.

import { describe, expect, it } from 'vitest'

/** Every source of the app, as text (vite reads them from disk). */
const sources = import.meta.glob('/src/**/*.{ts,tsx}', { query: '?raw', import: 'default', eager: true }) as Record<string, string>

const ENTRY = '/src/main.tsx'
const AVATAR = '/src/features/avatar/'
/** What draws or speaks: loaded with the overlay (or on demand), never with the app. */
const LAZY = ['Overlay.tsx', 'Figure.tsx', 'Bubble.tsx', 'Face.tsx', 'menu.ts', 'rig.ts', 'voices.ts', 'voiceOn.ts', 'lib/engine.ts', 'lib/speech.ts', 'lib/arbiter.ts', 'lib/look.ts', 'lib/builtin.ts', 'lib/defaultLook.ts'].map((f) => AVATAR + f)

/** The file a specifier names, seen from `from`; null for a package, a style sheet or anything else that is no source here. */
function resolve(from: string, spec: string): string | null {
  let path: string
  if (spec.startsWith('@/')) path = `/src/${spec.slice(2)}`
  else if (spec.startsWith('.')) {
    const parts = from.split('/').slice(0, -1)
    for (const part of spec.split('/')) {
      if (part === '.' || part === '') continue
      if (part === '..') parts.pop()
      else parts.push(part)
    }
    path = parts.join('/')
  } else return null
  path = path.replace(/\?.*$/, '')
  for (const candidate of [path, `${path}.ts`, `${path}.tsx`, `${path}/index.ts`, `${path}/index.tsx`]) if (Object.hasOwn(sources, candidate)) return candidate
  return null
}

/** The modules a source loads when it is loaded: its static imports and re-exports, without the type-only ones. */
function staticImports(file: string): string[] {
  const text = (sources[file] ?? '').replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
  const out: string[] = []
  for (const m of text.matchAll(/(?:^|\n)[ \t]*(?:import|export)\s+(type\s+)?(?:[^'";]*?\sfrom\s+)?['"]([^'"\n]+)['"]/g)) {
    if (m[1]) continue
    const target = resolve(file, m[2])
    if (target) out.push(target)
  }
  return out
}

/** Every source reachable from `entry` by static imports, each with the module that brought it in. */
function reach(entry: string): Map<string, string | null> {
  const seen = new Map<string, string | null>([[entry, null]])
  const queue = [entry]
  for (let file = queue.shift(); file !== undefined; file = queue.shift()) {
    for (const next of staticImports(file)) {
      if (seen.has(next)) continue
      seen.set(next, file)
      queue.push(next)
    }
  }
  return seen
}

/** How `file` came to be loaded: the chain of imports from the entry. */
function chain(seen: Map<string, string | null>, file: string): string {
  const path: string[] = []
  for (let at: string | null | undefined = file; at; at = seen.get(at)) path.unshift(at)
  return path.join(' -> ')
}

describe('what is loaded with the app', () => {
  const loaded = reach(ENTRY)

  it('is found by this test: the sources are read and the graph is walked', () => {
    expect(Object.keys(sources).length).toBeGreaterThan(300)
    for (const file of [ENTRY, ...LAZY]) expect(Object.hasOwn(sources, file), file).toBe(true)
    // The walk gets through the registry to every feature, the avatar's and the platform's among them.
    for (const file of ['/src/shell/registry.ts', '/src/features/avatar/index.ts', '/src/features/platform/index.ts', '/src/features/platform/MobileMore.tsx', '/src/features/agents/index.ts']) expect(loaded.has(file), file).toBe(true)
    // It tells a type-only import and a lazy one from a real one.
    expect(staticImports('/src/features/avatar/api.ts')).toContain('/src/features/avatar/lib/cache.ts')
    expect(staticImports('/src/features/avatar/api.ts')).not.toContain('/src/features/avatar/lib/arbiter.ts')
    expect(staticImports('/src/features/avatar/Provider.tsx')).not.toContain('/src/features/avatar/Overlay.tsx')
    expect(staticImports('/src/features/avatar/Overlay.tsx')).toContain('/src/features/avatar/Figure.tsx')
    // A list that spans lines is read whole.
    expect(staticImports('/src/features/platform/sections/Avatar.tsx')).toContain('/src/features/platform/avatarLib.ts')
  })

  it('holds the avatar’s provider, widget and commands', () => {
    for (const f of ['index.ts', 'Provider.tsx', 'StatusWidget.tsx', 'commands.ts', 'actions.ts', 'widget.ts', 'words.ts', 'queryAccess.ts', 'runtime.ts', 'store.ts', 'api.ts']) expect(loaded.has(AVATAR + f), f).toBe(true)
  })

  it('does not hold what draws or speaks: that is the overlay’s chunk, or loaded on demand', () => {
    for (const file of LAZY) expect(loaded.has(file) ? chain(loaded, file) : null, file).toBeNull()
  })

  it('reaches the overlay, the voice question and the phone’s Avatar section only through a lazy import', () => {
    expect(sources[AVATAR + 'Provider.tsx']).toMatch(/lazy\(\(\) => import\('\.\/Overlay'\)/)
    expect(sources[AVATAR + 'actions.ts']).toMatch(/\bimport\('\.\/voiceOn'\)/)
    expect(sources['/src/features/platform/MobileMore.tsx']).toMatch(/lazy\(\(\) => import\('\.\/sections\/Avatar'\)/)
    expect(loaded.has('/src/features/platform/sections/Avatar.tsx')).toBe(false)
    expect(loaded.has('/src/features/platform/SettingsPanel.tsx')).toBe(false)
  })

  it('lets the platform feature use only the two contract modules of the avatar slice', () => {
    const used = new Set<string>()
    for (const file of Object.keys(sources)) {
      if (!file.startsWith('/src/features/platform/') || file.endsWith('.test.ts')) continue
      for (const m of (sources[file] ?? '').matchAll(/from\s+['"](@\/features\/avatar[^'"]*)['"]/g)) used.add(m[1])
    }
    expect([...used].sort()).toEqual(['@/features/avatar/store', '@/features/avatar/voices'])
  })

  it('keeps the agents feature free of other features: it asks the avatar through the shell’s bridge', () => {
    for (const file of Object.keys(sources)) {
      if (!file.startsWith('/src/features/agents/') || file.endsWith('.test.ts')) continue
      expect(sources[file], file).not.toMatch(/from\s+['"]@\/features\/(?!agents\/)/)
    }
    expect(sources['/src/features/agents/AgentsHome.tsx']).toContain("from '@/shell/avatarBridge'")
    expect(sources['/src/features/agents/TerminalPanel.tsx']).toContain("from '@/shell/avatarBridge'")
  })
})
