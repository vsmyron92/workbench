import { CircleHelp } from 'lucide-react'
import { openHelp } from './actions'

/** Status bar item; hidden on a phone, which has Help under More. */
export function HelpButton() {
  return (
    <button className="wb-status-item" onClick={() => openHelp()} title="Help (F1)">
      <CircleHelp size={13} /> Help
    </button>
  )
}
