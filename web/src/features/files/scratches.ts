// Scratch files (CLion's Scratches): notes, requests and snippets outside every
// repository, in Workbench's own folder, reachable from any project. The server
// keeps them as the hidden project `wb-scratches` (server/src/projects.rs), so the
// editor, Local History and the HTTP Client work on them as on project files.

import { toastError } from '@/shell/actions'
import { showMenu, type MenuEntry } from '@/ui'
import { filesApi } from './api'
import { openFile, revealInTree } from './openers'
import { SCRATCH_ID } from './scratchStore'
import { refreshDirs } from './treeLoader'

export { isScratch, SCRATCH_ID } from './scratchStore'

export const SCRATCH_KINDS: { label: string; ext: string; template?: string }[] = [
  { label: 'Markdown', ext: 'md' },
  { label: 'Plain Text', ext: 'txt' },
  { label: 'HTTP Request', ext: 'http', template: '### \nGET http://localhost:8080/\nAccept: application/json\n\n' },
  { label: 'JSON', ext: 'json' },
  { label: 'YAML', ext: 'yaml' },
  { label: 'SQL', ext: 'sql' },
  { label: 'Shell Script', ext: 'sh', template: '#!/usr/bin/env bash\nset -euo pipefail\n\n' },
  { label: 'Python', ext: 'py' },
  { label: 'TypeScript', ext: 'ts' },
  { label: 'JavaScript', ext: 'js' },
  { label: 'Rust', ext: 'rs' },
  { label: 'Go', ext: 'go', template: 'package main\n\n' },
  { label: 'C', ext: 'c' },
  { label: 'C++', ext: 'cpp' },
  { label: 'Verilog', ext: 'v' },
  { label: 'SystemVerilog', ext: 'sv' },
  { label: 'VHDL', ext: 'vhd' },
  { label: 'TOML', ext: 'toml' },
  { label: 'XML', ext: 'xml' },
  { label: 'HTML', ext: 'html' },
  { label: 'CSS', ext: 'css' },
]

/** The first free `scratch.<ext>`, `scratch_2.<ext>`, … among `names` (CLion's naming). */
export function nextScratchName(names: Iterable<string>, ext: string): string {
  const taken = new Set([...names].map((n) => n.toLowerCase()))
  for (let n = 1; ; n++) {
    const name = n === 1 ? `scratch.${ext}` : `scratch_${n}.${ext}`
    if (!taken.has(name.toLowerCase())) return name
  }
}

/** Create a scratch file of a kind and open it. */
export async function newScratch(ext: string) {
  const kind = SCRATCH_KINDS.find((k) => k.ext === ext)
  try {
    const listing = await filesApi.list(SCRATCH_ID, '')
    let name = nextScratchName(
      listing.entries.map((e) => e.name),
      ext,
    )
    // `etag: null`: never over an existing file (another tab may have just taken the name).
    for (let tries = 0; ; tries++) {
      try {
        await filesApi.write(SCRATCH_ID, name, kind?.template ?? '', null)
        break
      } catch (e) {
        if (tries >= 3) throw e
        name = nextScratchName([...listing.entries.map((x) => x.name), name], ext)
        listing.entries.push({ name } as (typeof listing.entries)[number])
      }
    }
    refreshDirs(SCRATCH_ID, [''])
    revealInTree(SCRATCH_ID, name)
    openFile({ projectId: SCRATCH_ID, path: name })
  } catch (e) {
    toastError(e, 'Could not create the scratch file')
  }
}

/** CLion's New Scratch File popup: pick a language. */
export function chooseScratchKind(at?: { clientX: number; clientY: number }) {
  const items: MenuEntry[] = SCRATCH_KINDS.map((k) => ({ label: `${k.label}  .${k.ext}`, run: () => void newScratch(k.ext) }))
  showMenu(at ?? { clientX: Math.max(16, window.innerWidth / 2 - 110), clientY: 96 }, items)
}
