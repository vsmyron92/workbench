// The avatar feature's widget in the desktop's status bar: a face with the number of
// avatars this window shows, a crossed speaker while voice is off or not chosen, and
// "hidden" while avatars are hidden in this browser. Its menu stops the line being said,
// turns voice and the avatars on and off, gives a session of the project an avatar or
// turns it off, and opens the settings.
//
// Loaded with the app, so it draws no avatar: the face is the icon of the composer's
// Avatar control. What it shows and offers is decided in widget.ts.

import { useMemo } from 'react'
import { Smile, VolumeX } from 'lucide-react'
import { useTerminals } from '@/api/queries'
import { showMenu } from '@/ui'
import { giveAvatar, openAvatarSettings, setShow, setVoice, stopSpeaking, turnAvatarOff } from './actions'
import { useAvatars } from './api'
import { useAvatarRuntime } from './runtime'
import { useAvatarPrefs } from './store'
import { menuHeight, projectSessions, widgetMenu, widgetView, type WidgetActions, type WidgetInput } from './widget'
import { STATUS_WORDS } from './words'

const ACTIONS: WidgetActions = {
  stop: () => void stopSpeaking(),
  voice: setVoice,
  show: setShow,
  give: (session) => void giveAvatar(session),
  turnOff: (session) => void turnAvatarOff(session),
  settings: openAvatarSettings,
}

/** The menu's lower edge is this far above the widget, so that it clears the status bar's own edge. */
const MENU_GAP_PX = 6

/** Opens the menu above the widget: the status bar is the bottom edge of the window. */
function openMenu(el: HTMLElement, input: WidgetInput) {
  const items = widgetMenu(input, ACTIONS)
  const r = el.getBoundingClientRect()
  showMenu({ clientX: r.left, clientY: Math.max(4, r.top - MENU_GAP_PX - menuHeight(items)) }, items)
}

export function AvatarStatus({ projectId }: { projectId: string | null }) {
  const list = useAvatars().data
  const terminals = useTerminals().data
  const show = useAvatarPrefs((s) => s.show)
  const voice = useAvatarPrefs((s) => s.voice)
  const shown = useAvatarRuntime((s) => s.shown.length)
  const presenting = useAvatarRuntime((s) => s.presenting !== null)
  const sessions = useMemo(() => projectSessions(terminals, projectId), [terminals, projectId])
  const input: WidgetInput = { list, sessions, show, voice, shown, presenting }
  const view = widgetView(input)
  if (!view.visible) return null
  return (
    <button
      type="button"
      className="wb-status-item wb-av-status"
      aria-haspopup="menu"
      aria-label={view.title}
      title={view.title}
      // A click leaves focus where it was (the terminal): the menu gives it back there when it closes.
      onMouseDown={(e) => e.preventDefault()}
      onClick={(e) => openMenu(e.currentTarget, input)}
    >
      <Smile size={13} />
      {view.hidden ? <span>{STATUS_WORDS.hidden}</span> : view.count > 0 ? <span>{view.count}</span> : null}
      {view.muted && <VolumeX size={12} />}
    </button>
  )
}
