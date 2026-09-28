// A page's labels in its header: shown as chips, removed with ×, added inline.

import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Plus, Tag, X } from 'lucide-react'
import { toastError } from '@/shell/actions'
import { Input, Spinner } from '@/ui'
import { confluenceApi, qk, type Page } from '../api'
import { parseLabels } from '../links'

export function Labels({ projectId, page, readOnly }: { projectId: string | null; page: Page; readOnly: boolean }) {
  const qc = useQueryClient()
  const [adding, setAdding] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: qk.page(projectId, page.id) })

  const add = async () => {
    const names = parseLabels(adding ?? '').filter((n) => !page.labels.includes(n))
    if (!names.length) return setAdding(null)
    setBusy('+')
    try {
      await confluenceApi.addLabels(projectId, page.id, names)
      setAdding(null)
      await refresh()
    } catch (e) {
      toastError(e, 'Could not add the label')
    } finally {
      setBusy(null)
    }
  }

  const remove = async (name: string) => {
    setBusy(name)
    try {
      await confluenceApi.removeLabel(projectId, page.id, name)
      await refresh()
    } catch (e) {
      toastError(e, `Could not remove “${name}”`)
    } finally {
      setBusy(null)
    }
  }

  if (readOnly && !page.labels.length) return null
  return (
    <span className="cf-labels" aria-label="Labels">
      <Tag size={12} className="wb-subtle" />
      {page.labels.map((l) => (
        <span key={l} className="cf-label">
          {l}
          {!readOnly &&
            (busy === l ? (
              <Spinner size={9} />
            ) : (
              <button className="x" title={`Remove label “${l}”`} aria-label={`Remove label ${l}`} onClick={() => void remove(l)}>
                <X size={10} />
              </button>
            ))}
        </span>
      ))}
      {!readOnly &&
        (adding !== null ? (
          <Input
            small
            autoFocus
            className="cf-label-input"
            value={adding}
            placeholder="label, another"
            disabled={busy === '+'}
            onChange={(e) => setAdding(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void add()
              if (e.key === 'Escape') setAdding(null)
            }}
            onBlur={() => (adding.trim() ? void add() : setAdding(null))}
            aria-label="New labels"
          />
        ) : (
          <button className="cf-label add" onClick={() => setAdding('')} title="Add labels">
            <Plus size={11} /> {page.labels.length ? '' : 'Label'}
          </button>
        ))}
    </span>
  )
}
