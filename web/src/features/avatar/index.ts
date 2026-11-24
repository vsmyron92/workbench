// Feature slice: avatar. Owned by the avatar slice: see docs/ARCHITECTURE.md.
// A hosted agent session can have a live avatar: a small animated character over the
// Workbench window that the session draws itself, that says its short messages with
// captions, asks the user questions and takes the answer, and mirrors the session's state.
//
// Loaded with the app: the provider (the list, the bridge to the agents feature), the
// place of the compact face in the agents window's top bar, the status-bar widget and the
// palette commands. Everything that draws or speaks is the overlay, a lazy chunk the
// provider mounts when there is an avatar to show: nothing reachable from here imports a
// look, the engine or speech.

import type { FeatureModule } from '@/shell/types'
import { avatarCommands } from './commands'
import { AvatarBarSlot, AvatarProvider } from './Provider'
import { AvatarStatus } from './StatusWidget'

const feature: FeatureModule = {
  id: 'avatar',
  commands: avatarCommands,
  // Desktop, workspace window collapsed: the active avatar is a face here, at the right end of the agents window's top bar.
  columnbar: [AvatarBarSlot],
  // Desktop: how many avatars are shown and whether they speak, with the menu that gives a session one.
  statusbar: [AvatarStatus],
  providers: [AvatarProvider],
}

export default feature
