import { describe, expect, it } from 'vitest'
import { migrateUi, useUi } from './store'

describe('stored layout of earlier versions', () => {
  const sides = (left: string | null, bottom: string | null) => ({
    left: { active: left, size: 310 },
    right: { active: 'gitlab', size: 380 },
    bottom: { active: bottom, size: 260 },
  })

  it('opens on the Workspace cards and drops the Terminal tool window, which is a column now', () => {
    const out = migrateUi({ projectId: 'shop', sides: sides('agents', 'terminal') }, 0)
    expect(out.sides).toEqual(sides('workspace', null))
    expect(out.projectId).toBe('shop')
    // Other bottom tool windows stay open, and sizes are kept.
    expect(migrateUi({ sides: sides('files', 'gitlog') }, 0).sides).toEqual(sides('workspace', 'gitlog'))
  })

  it('leaves the current version and states without a layout alone', () => {
    const now = { sides: sides('files', 'terminal') }
    expect(migrateUi(now, 1)).toBe(now)
    expect(migrateUi({ projectId: 'shop' }, 0)).toEqual({ projectId: 'shop' })
  })
})

describe('the workspace window', () => {
  it('opens when any tool window is shown or toggled on, and keeps the state the user left when one is hidden', () => {
    const ui = useUi.getState()
    ui.setWorkOpen(false)
    ui.showToolWindow('left', 'files')
    expect(useUi.getState().workOpen).toBe(true)
    ui.setWorkOpen(false)
    ui.toggleToolWindow('left', 'files') // hides it: the window stays as the user left it
    expect([useUi.getState().sides.left.active, useUi.getState().workOpen]).toEqual([null, false])
    ui.toggleToolWindow('right', 'apps')
    expect([useUi.getState().sides.right.active, useUi.getState().workOpen]).toEqual(['apps', true])
  })
})
