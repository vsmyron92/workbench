// A folder picker for "Add project": lists the subfolders of a directory on the
// Workbench computer (`GET /api/fs/dirs`). Clicking a folder enters it; "Use this
// folder" hands its path to the caller.

import { useEffect, useState } from 'react'
import { ArrowUp, Folder, FolderOpen, Home } from 'lucide-react'
import { api } from '@/api/client'
import { Button, ErrorBox, IconButton, Loading } from '@/ui'

interface DirList {
  path: string
  parent: string | null
  home: string
  dirs: string[]
  truncated: boolean
}

/** `dir` and `name` joined with the separator `dir` already uses. */
export function joinPath(dir: string, name: string): string {
  const sep = dir.includes('\\') && !dir.includes('/') ? '\\' : '/'
  return dir.endsWith(sep) ? dir + name : dir + sep + name
}

export function FolderBrowser({ start, onPick }: { start: string; onPick: (path: string) => void }) {
  const [path, setPath] = useState(start)
  const [hidden, setHidden] = useState(false)
  const [list, setList] = useState<DirList | null>(null)
  const [error, setError] = useState<unknown>(null)

  useEffect(() => {
    const ctl = new AbortController()
    setError(null)
    api
      .get<DirList>('/api/fs/dirs', { path, hidden: hidden || undefined }, ctl.signal)
      .then(setList)
      .catch((e) => {
        if (!ctl.signal.aborted) setError(e)
      })
    return () => ctl.abort()
  }, [path, hidden])

  return (
    <div className="wb-folder-browser">
      <div className="wb-folder-bar">
        <IconButton icon={ArrowUp} size="small" label="Parent folder" disabled={!list?.parent} onClick={() => list?.parent && setPath(list.parent)} />
        <IconButton icon={Home} size="small" label="Home folder" onClick={() => setPath(list?.home ?? '~')} />
        <span className="wb-grow wb-ellipsis mono wb-small" title={list?.path}>
          {list?.path ?? path}
        </span>
        <label className="wb-small wb-muted">
          <input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} /> Hidden
        </label>
      </div>
      <div className="wb-folder-list wb-scroll">
        {error ? (
          <ErrorBox error={error} onRetry={() => setPath('~')} />
        ) : !list ? (
          <Loading />
        ) : list.dirs.length === 0 ? (
          <div className="wb-small wb-muted wb-folder-empty">No subfolders</div>
        ) : (
          list.dirs.map((d) => (
            <div key={d} className="wb-list-row" role="button" tabIndex={0} onClick={() => setPath(joinPath(list.path, d))} onKeyDown={(e) => e.key === 'Enter' && setPath(joinPath(list.path, d))}>
              <Folder size={14} />
              <span className="wb-ellipsis">{d}</span>
            </div>
          ))
        )}
        {list?.truncated && <div className="wb-small wb-muted wb-folder-empty">Only the first folders are shown: type the path instead.</div>}
      </div>
      <Button size="small" variant="primary" icon={FolderOpen} disabled={!list} onClick={() => list && onPick(list.path)}>
        Use this folder
      </Button>
    </div>
  )
}
