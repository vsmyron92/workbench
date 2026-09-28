// Create a Confluence page (in a space, optionally under a parent), from nothing or
// from markdown, then open it in the editor.

import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button, ErrorBox, Field, Input, Modal, Select, Tabs, TextArea } from '@/ui'
import { useProject } from '@/api/queries'
import { useUi } from '@/state/store'
import { confluenceApi, useSpaces } from '../api'
import { confluenceLinks } from '../links'
import { useAtlassianUi, usePrefs } from '../state'
import { openConfluencePage } from './actions'

export function NewPageDialog() {
  const req = useAtlassianUi((s) => s.newPage)
  const close = () => useAtlassianUi.getState().openNewPage(null)
  if (!req) return null
  return <NewPageForm key={JSON.stringify(req)} req={req} onClose={close} />
}

function NewPageForm({ req, onClose }: { req: NonNullable<ReturnType<typeof useAtlassianUi.getState>['newPage']>; onClose: () => void }) {
  const qc = useQueryClient()
  const projectId = useUi((s) => s.projectId)
  const project = useProject(projectId)
  const links = confluenceLinks(project.data?.config)
  const spaces = useSpaces(projectId)
  const remembered = usePrefs((s) => s.spaceByProject[projectId ?? ''])
  const defaultSpace = req.spaceId ?? remembered ?? spaces.data?.find((s) => s.key === links.space)?.id ?? spaces.data?.[0]?.id ?? ''
  const [spaceId, setSpaceId] = useState<string>('')
  const [title, setTitle] = useState('')
  const [body, setBody] = useState<'blank' | 'markdown'>('blank')
  const [markdown, setMarkdown] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const effectiveSpace = spaceId || defaultSpace
  const underParent = !!req.parentId

  const create = async () => {
    if (!title.trim()) return
    setBusy(true)
    setError(null)
    try {
      const out = await confluenceApi.create(projectId, {
        title: title.trim(),
        spaceId: underParent ? undefined : effectiveSpace || undefined,
        parentId: req.parentId,
        ...(body === 'markdown' && markdown.trim() ? { markdown } : { storage: '' }),
      })
      onClose()
      qc.invalidateQueries({ predicate: (q) => (q.queryKey as unknown[])[0] === 'confluence' && ['children', 'roots'].includes(String((q.queryKey as unknown[])[1])) })
      openConfluencePage(out.id, out.title, { mode: body === 'markdown' ? 'view' : 'edit' })
    } catch (e) {
      setError(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      title={underParent ? `New page under “${req.parentTitle ?? req.parentId}”` : 'New Confluence page'}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!title.trim() || (!underParent && !effectiveSpace)} onClick={() => void create()}>
            Create
          </Button>
        </>
      }
    >
      {!underParent && (
        <Field label="Space" hint="New pages go under the space's home page.">
          <Select value={effectiveSpace} onChange={(e) => setSpaceId(e.target.value)}>
            {(spaces.data ?? []).map((s) => (
              <option key={s.id} value={s.id}>
                {s.name} {s.type === 'personal' ? '(personal)' : `· ${s.key}`}
              </option>
            ))}
          </Select>
        </Field>
      )}
      <Field label="Title">
        <Input autoFocus value={title} placeholder="Page title" onChange={(e) => setTitle(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && body === 'blank' && void create()} />
      </Field>
      <Tabs
        tabs={[
          { id: 'blank', label: 'Blank (open the editor)' },
          { id: 'markdown', label: 'From markdown' },
        ]}
        value={body}
        onChange={setBody}
      />
      {body === 'markdown' && (
        <TextArea rows={10} value={markdown} placeholder={'# Heading\n\nSome **markdown** — fenced code becomes the code macro.'} onChange={(e) => setMarkdown(e.target.value)} className="mono" />
      )}
      {error !== null && <ErrorBox error={error} />}
    </Modal>
  )
}
