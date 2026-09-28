// "Create devcontainer.json…": a starter config proposed from the detected stack, in
// an editable preview. Written into the repository only on Create, never over a file.

import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { create } from 'zustand'
import { FilePlus2 } from 'lucide-react'
import { Badge, Button, DialogBoundary, ErrorBox, Input, Loading, Modal, TextArea } from '@/ui'
import { fetchProposal, openDevcontainerPanel, writeProposal } from './api'

const useScaffold = create<{ pid: string | null; set: (pid: string | null) => void }>()((set) => ({ pid: null, set: (pid) => set({ pid }) }))

export function openScaffoldDialog(pid: string) {
  useScaffold.getState().set(pid)
}

export function ScaffoldDialogHost() {
  const { pid, set } = useScaffold()
  if (!pid) return null
  return (
    <DialogBoundary key={pid} title="Create devcontainer.json" onClose={() => set(null)}>
      <ScaffoldDialog pid={pid} onClose={() => set(null)} />
    </DialogBoundary>
  )
}

function ScaffoldDialog({ pid, onClose }: { pid: string; onClose: () => void }) {
  const q = useQuery({ queryKey: ['devcontainer', 'scaffold', pid], queryFn: () => fetchProposal(pid), staleTime: 0, gcTime: 0, retry: false })
  const [content, setContent] = useState('')
  const [path, setPath] = useState('.devcontainer/devcontainer.json')
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    if (q.data) {
      setContent(q.data.content)
      setPath(q.data.path)
    }
  }, [q.data])

  const create = async () => {
    setBusy(true)
    const ok = await writeProposal(pid, path.trim(), content)
    setBusy(false)
    if (ok) {
      onClose()
      openDevcontainerPanel(pid)
    }
  }

  return (
    <Modal
      wide
      title={
        <span className="wb-row">
          <FilePlus2 size={16} /> Create devcontainer.json
          {q.data?.stacks.map((s) => (
            <Badge key={s} tone="accent">
              {s}
            </Badge>
          ))}
        </span>
      }
      onClose={onClose}
      footer={
        <>
          <span className="wb-xs wb-subtle">Nothing is written until you press Create; an existing file is never replaced.</span>
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={FilePlus2} loading={busy} disabled={!content.trim() || !path.trim() || q.data?.exists} onClick={() => void create()}>
            Create
          </Button>
        </>
      }
    >
      {q.isLoading && <Loading label="Looking at the repository…" />}
      {q.error && <ErrorBox error={q.error} onRetry={() => void q.refetch()} />}
      {q.data && (
        <div className="wb-dc-scaffold">
          {q.data.exists && <div className="wb-error setup">This project already has a devcontainer.json; Workbench does not replace it.</div>}
          <div className="wb-row">
            <label className="wb-small wb-muted" htmlFor="wb-dc-path">
              File
            </label>
            <Input id="wb-dc-path" small className="mono wb-grow" value={path} onChange={(e) => setPath(e.target.value)} />
          </div>
          <TextArea className="mono wb-dc-editor" spellCheck={false} rows={20} value={content} onChange={(e) => setContent(e.target.value)} aria-label="devcontainer.json" />
          {q.data.notes.map((n) => (
            <div key={n} className="wb-xs wb-muted">
              {n}
            </div>
          ))}
        </div>
      )}
    </Modal>
  )
}
