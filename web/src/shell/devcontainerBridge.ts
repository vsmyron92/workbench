// Dev container actions other slices offer (the Apps tool window, the phone's Apps
// tab, terminals): the devcontainer feature registers the handler, so a Start from
// anywhere goes through its confirmation dialog, which lists exactly what will run.

import { toast } from './actions'

export type DevcontainerAction = 'panel' | 'start' | 'stop' | 'rebuild' | 'shell'

type Handler = (projectId: string, action: DevcontainerAction) => void

let handler: Handler | null = null

/** Called once by the devcontainer feature's provider. */
export function setDevcontainerHandler(h: Handler | null) {
  handler = h
}

export function devcontainerAction(projectId: string, action: DevcontainerAction) {
  if (handler) handler(projectId, action)
  else toast('warning', 'Dev containers are not available')
}
