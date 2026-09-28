// A refresh failed while an earlier copy is on screen: keep showing that copy (and
// whatever is being written next to it) and say so, instead of replacing it all with
// an error box.

import { AlertTriangle, RefreshCw } from 'lucide-react'
import { Button } from '@/ui'

export function StaleNotice({ error, onRetry, what }: { error: unknown; onRetry: () => void; what: string }) {
  const msg = error instanceof Error ? error.message : String(error)
  return (
    <div className="cf-banner" role="status">
      <AlertTriangle size={14} className="wb-warning" style={{ flex: 'none' }} />
      <span className="wb-grow wb-ellipsis" title={msg}>
        Could not refresh {what} ({msg}); showing the last copy.
      </span>
      <Button size="small" variant="ghost" icon={RefreshCw} onClick={onRetry}>
        Retry
      </Button>
    </div>
  )
}
