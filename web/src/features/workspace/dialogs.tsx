// Dialogs: new card, "Open card…" picker, add/edit step, card details.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Search, Upload } from 'lucide-react'
import { useProjects } from '@/api/queries'
import { toast, toastError } from '@/shell/actions'
import { Button, EmptyState, Field, Input, Loading, Modal, Select, TextArea } from '@/ui'
import { useCardFiles, useCards, VIEWERS, wsApi, type WorkspaceCard, type WorkspaceStep } from './api'
import { applyCard, openCard, patchCard } from './actions'
import { ALL, basename, categoryLabel, HOME, matchesQuery, relativeDay } from './logic'
import { CardThumb } from './parts'
import { useWsUi } from './store'

function useCategories(scope: string): string[] {
  const { data } = useCards(ALL)
  return useMemo(() => {
    const cats = new Set<string>(['research', 'report', 'design', 'analytics', 'dev'])
    for (const c of data?.cards ?? []) if (c.category && (c.scope === scope || scope === ALL)) cats.add(c.category)
    return [...cats].sort()
  }, [data, scope])
}

export function NewCardDialog({ initialScope, onClose }: { initialScope: string; onClose: () => void }) {
  const qc = useQueryClient()
  const { data: projects } = useProjects()
  const [scope, setScope] = useState(initialScope)
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const [category, setCategory] = useState('')
  const [busy, setBusy] = useState(false)
  const categories = useCategories(scope)
  const create = async () => {
    if (!title.trim() || busy) return
    setBusy(true)
    try {
      const c = await wsApi.create(scope, { title: title.trim(), description: description.trim(), category: category.trim() })
      applyCard(qc, c)
      openCard(c.scope, c.id, c.title)
      toast('success', `Created "${c.title}"`)
      onClose()
    } catch (e) {
      toastError(e, 'Could not create the card')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="New card"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!title.trim()} onClick={() => void create()}>
            Create
          </Button>
        </>
      }
    >
      <form
        className="ws-form"
        onSubmit={(e) => {
          e.preventDefault()
          void create()
        }}
      >
        <Field label="Title">
          <Input autoFocus value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Q3 load analysis" maxLength={200} />
        </Field>
        <Field label="Description" hint="What this card collects and why.">
          <TextArea rows={3} value={description} onChange={(e) => setDescription(e.target.value)} maxLength={4000} />
        </Field>
        <div className="ws-form-row">
          <Field label="Category">
            <Input list="ws-categories" value={category} onChange={(e) => setCategory(e.target.value)} placeholder="research" maxLength={40} />
            <datalist id="ws-categories">
              {categories.map((c) => (
                <option key={c} value={c} />
              ))}
            </datalist>
          </Field>
          <Field label="Scope">
            <Select value={scope} onChange={(e) => setScope(e.target.value)}>
              {(projects ?? []).map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
              <option value={HOME}>Home (not tied to a project)</option>
            </Select>
          </Field>
        </div>
        <button type="submit" hidden />
      </form>
    </Modal>
  )
}

/** "Open card…": search every scope's cards (archive included). */
export function CardPicker({ onClose }: { onClose: () => void }) {
  const { data, isLoading } = useCards(ALL)
  const [query, setQuery] = useState('')
  const [sel, setSel] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const list = useMemo(() => {
    const cards = data?.cards ?? []
    const q = query.trim()
    return (q ? cards.filter((c) => matchesQuery(c, q)) : cards.filter((c) => !c.archived)).slice(0, 200)
  }, [data, query])
  useEffect(() => setSel(0), [query])
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>('.selected')?.scrollIntoView({ block: 'nearest' })
  }, [sel])
  const open = (c: WorkspaceCard | undefined) => {
    if (!c) return
    openCard(c.scope, c.id, c.title)
    onClose()
  }
  return (
    <Modal title="Open card" onClose={onClose} wide>
      <div className="ws-search wide">
        <Search size={14} />
        <input
          autoFocus
          value={query}
          placeholder="Search cards in every scope (archive included)"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown') {
              e.preventDefault()
              setSel((s) => Math.min(s + 1, list.length - 1))
            } else if (e.key === 'ArrowUp') {
              e.preventDefault()
              setSel((s) => Math.max(s - 1, 0))
            } else if (e.key === 'Enter') {
              e.preventDefault()
              open(list[sel])
            }
          }}
        />
      </div>
      <div className="ws-picker" ref={listRef}>
        {isLoading ? (
          <Loading />
        ) : !list.length ? (
          <EmptyState title={query ? 'No cards match' : 'No cards yet'} />
        ) : (
          list.map((c, i) => (
            <div key={`${c.scope}:${c.id}`} className={`wb-list-row ws-picker-row${i === sel ? ' selected' : ''}`} onMouseEnter={() => setSel(i)} onClick={() => open(c)}>
              <CardThumb card={c} size={28} />
              <div className="wb-grow">
                <div className="wb-ellipsis">{c.title}</div>
                <div className="wb-xs wb-muted wb-ellipsis">
                  {c.scopeName} · {categoryLabel(c.category)} · {relativeDay(c.touchedAt)}
                  {c.archived ? ' · archived' : ''}
                </div>
              </div>
            </div>
          ))
        )}
      </div>
    </Modal>
  )
}

/** Add a step from the card folder (or upload files as steps), or edit one. */
export function StepDialog({ card, step, onClose }: { card: WorkspaceCard; step?: WorkspaceStep; onClose: () => void }) {
  const qc = useQueryClient()
  const { data: files, isLoading } = useCardFiles(card.scope, card.id, '')
  const [path, setPath] = useState(step?.path ?? '')
  const [name, setName] = useState(step?.name ?? '')
  const [nameTouched, setNameTouched] = useState(!!step)
  const [viewer, setViewer] = useState(step?.viewer ?? 'auto')
  const [busy, setBusy] = useState(false)
  const fileInput = useRef<HTMLInputElement>(null)
  const taken = new Set(card.steps.map((s) => s.path))
  const choices = (files?.entries ?? []).filter((f) => !taken.has(f.path) || f.path === step?.path)

  const submit = async () => {
    if (!path.trim() || busy) return
    setBusy(true)
    try {
      const c = step
        ? await wsApi.patchStep(card.scope, card.id, step.index, { name: name.trim() || undefined, viewer: viewer === 'auto' ? null : viewer, expectPath: step.path })
        : await wsApi.addStep(card.scope, card.id, { name: name.trim(), path: path.trim(), viewer: viewer === 'auto' ? undefined : viewer })
      applyCard(qc, c)
      onClose()
    } catch (e) {
      toastError(e, step ? 'Could not change the step' : 'Could not add the step')
    } finally {
      setBusy(false)
    }
  }

  const upload = async (list: FileList | null) => {
    if (!list?.length) return
    setBusy(true)
    let last: WorkspaceCard | null = null
    for (const f of Array.from(list)) {
      try {
        last = (await wsApi.upload(card.scope, card.id, f, f.name, { step: true })).card
      } catch (e) {
        toastError(e, `Could not upload ${f.name}`)
      }
    }
    setBusy(false)
    if (last) {
      applyCard(qc, last)
      onClose()
    }
  }

  return (
    <Modal
      title={step ? 'Edit step' : 'Add step'}
      onClose={onClose}
      footer={
        <>
          {!step && (
            <>
              <input ref={fileInput} type="file" multiple hidden onChange={(e) => void upload(e.target.files)} />
              <Button icon={Upload} onClick={() => fileInput.current?.click()} disabled={busy} title="Upload files from this device; each becomes a step">
                Upload files…
              </Button>
              <span style={{ flex: 1 }} />
            </>
          )}
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!path.trim()} onClick={() => void submit()}>
            {step ? 'Save' : 'Add'}
          </Button>
        </>
      }
    >
      <form
        className="ws-form"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <Field
          label="File or folder"
          hint={step ? undefined : 'Relative to the card folder, or an absolute path in a project (copied into the card). A folder of images becomes a gallery.'}
        >
          {step ? (
            <Input value={path} disabled />
          ) : (
            <>
              <Input
                autoFocus
                list="ws-step-files"
                value={path}
                placeholder={isLoading ? 'Loading files…' : 'report.html'}
                onChange={(e) => {
                  setPath(e.target.value)
                  if (!nameTouched) setName(basename(e.target.value).replace(/\.[^.]+$/, ''))
                }}
              />
              <datalist id="ws-step-files">
                {choices.map((f) => (
                  <option key={f.path} value={f.path}>
                    {f.dir ? 'folder' : f.kind}
                  </option>
                ))}
              </datalist>
            </>
          )}
        </Field>
        <div className="ws-form-row">
          <Field label="Tab name">
            <Input
              value={name}
              onChange={(e) => {
                setName(e.target.value)
                setNameTouched(true)
              }}
              maxLength={120}
            />
          </Field>
          <Field label="Viewer">
            <Select value={viewer} onChange={(e) => setViewer(e.target.value)}>
              {VIEWERS.map((v) => (
                <option key={v} value={v}>
                  {v === 'auto' ? 'Automatic (by file type)' : v}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <button type="submit" hidden />
      </form>
    </Modal>
  )
}

export function EditCardDialog({ card, onClose }: { card: WorkspaceCard; onClose: () => void }) {
  const qc = useQueryClient()
  const [title, setTitle] = useState(card.title)
  const [description, setDescription] = useState(card.description)
  const [category, setCategory] = useState(card.category)
  const [busy, setBusy] = useState(false)
  const categories = useCategories(card.scope)
  const save = async () => {
    if (!title.trim()) return
    setBusy(true)
    const c = await patchCard(qc, card, { title: title.trim(), description: description.trim(), category: category.trim() })
    setBusy(false)
    if (c) onClose()
  }
  return (
    <Modal
      title="Card details"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={busy} disabled={!title.trim()} onClick={() => void save()}>
            Save
          </Button>
        </>
      }
    >
      <form
        className="ws-form"
        onSubmit={(e) => {
          e.preventDefault()
          void save()
        }}
      >
        <Field label="Title">
          <Input autoFocus value={title} onChange={(e) => setTitle(e.target.value)} maxLength={200} />
        </Field>
        <Field label="Description">
          <TextArea rows={4} value={description} onChange={(e) => setDescription(e.target.value)} maxLength={4000} />
        </Field>
        <Field label="Category">
          <Input list="ws-edit-categories" value={category} onChange={(e) => setCategory(e.target.value)} maxLength={40} />
          <datalist id="ws-edit-categories">
            {categories.map((c) => (
              <option key={c} value={c} />
            ))}
          </datalist>
        </Field>
        <button type="submit" hidden />
      </form>
    </Modal>
  )
}

/** Mounted once: the dialogs opened from commands and panels. */
export function WorkspaceDialogs() {
  const newCardScope = useWsUi((s) => s.newCardScope)
  const pickerOpen = useWsUi((s) => s.pickerOpen)
  const closeNewCard = useWsUi((s) => s.closeNewCard)
  const setPicker = useWsUi((s) => s.setPicker)
  return (
    <>
      {newCardScope !== null && <NewCardDialog initialScope={newCardScope} onClose={closeNewCard} />}
      {pickerOpen && <CardPicker onClose={() => setPicker(false)} />}
    </>
  )
}
