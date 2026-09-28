// The Services tool window (bottom, Alt+8; CLion's Services › Docker): this computer's
// containers, grouped by compose project, and images, on the left; the selection's
// details and actions on the right. Logs and shells open as terminals. Changes arrive
// as `docker.changed` from the server's `docker events` watcher.

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  Boxes,
  Box,
  ChevronDown,
  ChevronRight,
  Container,
  Copy,
  ExternalLink,
  Eraser,
  FileCode,
  Layers,
  Pause,
  Play,
  RefreshCw,
  RotateCw,
  ScrollText,
  Skull,
  Square,
  SquareTerminal,
  Trash2,
} from 'lucide-react'
import { useInvalidateOn } from '@/api/events'
import { useProjects } from '@/api/queries'
import type { ProjectSummary } from '@/api/types'
import { confirmDialog, openPanel, toast } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Badge, Button, EmptyState, ErrorBox, IconButton, Input, Loading, MonacoEditor, Splitter, StatusDot, Tabs, TimeAgo, formatBytes, showMenu, type MenuEntry } from '@/ui'
import { openDevcontainerPanel } from '../api'
import {
  composeAction,
  containerAction,
  dockerKeys,
  openShell,
  pruneImages,
  removeImage,
  showLogs,
  useContainer,
  useContainers,
  useImage,
  useImages,
  type ComposeInfo,
  type DockerContainer,
  type DockerImage,
  type PortMap,
} from './api'
import { buildTree, containerTone, dockerTime, filterTree, imageName, imageRef, isRunning, portLabel, portUrl, relativeTo, serviceLabel, shortId, type TreeNode } from './logic'
import './services.css'

type Row =
  | { key: string; kind: 'header'; section: 'containers' | 'images'; count: number }
  | { key: string; kind: 'compose'; node: Extract<TreeNode, { kind: 'compose' }> }
  | { key: string; kind: 'container'; c: DockerContainer; label: string; depth: number }
  | { key: string; kind: 'image'; img: DockerImage }

const TREE_MIN = 220

export function ServicesToolWindow({ projectId }: { projectId: string | null }) {
  const qc = useQueryClient()
  useInvalidateOn(qc, ['docker.changed'], () => dockerKeys.all)
  const containers = useContainers()
  const images = useImages()
  const projects = useProjects().data
  const [scope, setScope] = useState<'all' | 'project'>('all')
  const [filter, setFilter] = useState('')
  const [closed, setClosed] = useState<Set<string>>(new Set())
  const [sel, setSel] = useState<string | null>(null)
  const [width, setWidth] = useState(360)
  const startWidth = useRef(width)
  const treeRef = useRef<HTMLDivElement>(null)

  const tree = useMemo(() => buildTree(containers.data?.containers ?? [], projectId), [containers.data, projectId])
  const shown = useMemo(() => filterTree(tree, scope === 'project' ? projectId : null, filter), [tree, scope, projectId, filter])
  const imageList = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    return (images.data?.images ?? []).filter((i) => !needle || imageName(i).toLowerCase().includes(needle))
  }, [images.data, filter])

  const rows = useMemo(() => {
    const out: Row[] = []
    const count = shown.reduce((n, x) => n + (x.kind === 'compose' ? x.containers.length : 1), 0)
    out.push({ key: 'h:containers', kind: 'header', section: 'containers', count })
    if (!closed.has('h:containers')) {
      for (const n of shown) {
        if (n.kind === 'container') {
          out.push({ key: `c:${n.c.id}`, kind: 'container', c: n.c, label: n.c.name, depth: 1 })
          continue
        }
        const key = `g:${n.project}`
        out.push({ key, kind: 'compose', node: n })
        if (!closed.has(key)) for (const c of n.containers) out.push({ key: `c:${c.id}`, kind: 'container', c, label: serviceLabel(c, n.containers), depth: 2 })
      }
    }
    if (scope === 'all') {
      out.push({ key: 'h:images', kind: 'header', section: 'images', count: imageList.length })
      if (!closed.has('h:images')) for (const img of imageList) out.push({ key: `i:${img.id}:${imageName(img)}`, kind: 'image', img })
    }
    return out
  }, [shown, closed, imageList, scope])

  const selected = rows.find((r) => r.key === sel) ?? null
  const toggle = useCallback(
    (key: string, open?: boolean) =>
      setClosed((s) => {
        const n = new Set(s)
        if (open ?? n.has(key)) n.delete(key)
        else n.add(key)
        return n
      }),
    [],
  )

  // Keep the selected row in view as the keyboard moves it.
  useEffect(() => {
    if (!sel) return
    treeRef.current?.querySelector(`[data-key="${CSS.escape(sel)}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [sel])

  const onKeyDown = (e: KeyboardEvent) => {
    const i = rows.findIndex((r) => r.key === sel)
    const r = rows[i]
    const move = (to: number) => {
      const next = rows[Math.max(0, Math.min(rows.length - 1, to))]
      if (next) setSel(next.key)
    }
    const group = r && (r.kind === 'header' || r.kind === 'compose')
    if (e.key === 'ArrowDown') move(i < 0 ? 0 : i + 1)
    else if (e.key === 'ArrowUp') move(i < 0 ? 0 : i - 1)
    else if (e.key === 'Home') move(0)
    else if (e.key === 'End') move(rows.length - 1)
    else if (e.key === 'ArrowRight' && group && closed.has(r.key)) toggle(r.key, true)
    else if (e.key === 'ArrowLeft' && group && !closed.has(r.key)) toggle(r.key, false)
    else if (e.key === 'ArrowLeft' && r) {
      // To the parent row.
      for (let j = i - 1; j >= 0; j--) {
        const p = rows[j]
        if (p.kind === 'header' || (p.kind === 'compose' && r.kind === 'container' && r.depth === 2)) return setSel(p.key)
      }
    } else if (e.key === 'Enter' && r?.kind === 'container') void showLogs(r.c.id, projectId)
    else if (e.key === 'Enter' && group) toggle(r.key)
    else if (e.key === 'Delete' && r?.kind === 'container') void confirmRemoveContainer(r.c)
    else if (e.key === 'Delete' && r?.kind === 'image') void confirmRemoveImage(r.img)
    else return
    e.preventDefault()
  }

  const error = containers.data?.error
  const current = projects?.find((p) => p.id === projectId) ?? null

  return (
    <div className="wb-fill wb-svc">
      <div className="wb-svc-tree" style={{ width }}>
        <div className="wb-svc-bar">
          <Input small className="wb-svc-filter" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter containers and images" />
          <IconButton
            icon={Boxes}
            size="small"
            label={scope === 'project' ? 'Showing this project’s containers: show all' : 'Show only this project’s containers'}
            active={scope === 'project'}
            disabled={!projectId}
            onClick={() => setScope((s) => (s === 'all' ? 'project' : 'all'))}
          />
          <IconButton icon={RefreshCw} size="small" label="Refresh" onClick={() => void qc.invalidateQueries({ queryKey: dockerKeys.all })} />
        </div>
        {error ? (
          <EmptyState icon={Container} title="Docker is not available">
            {error}
          </EmptyState>
        ) : containers.error ? (
          <ErrorBox error={containers.error} onRetry={() => void containers.refetch()} />
        ) : !containers.data ? (
          <Loading label="Asking Docker…" />
        ) : (
          <div ref={treeRef} className="wb-scroll wb-svc-rows" role="tree" aria-label="Docker" tabIndex={0} onKeyDown={onKeyDown}>
            {rows.map((r) => (
              <TreeRow key={r.key} row={r} selected={r.key === sel} open={!closed.has(r.key)} projectId={projectId} onSelect={() => setSel(r.key)} onToggle={() => toggle(r.key)} />
            ))}
            {scope === 'project' && !shown.length && <div className="wb-svc-note">No container of this project. Compose projects and dev containers whose folder is inside it count.</div>}
          </div>
        )}
      </div>
      <Splitter
        direction="v"
        onResizeStart={() => (startWidth.current = width)}
        onResize={(d) => setWidth(Math.max(TREE_MIN, Math.min(900, startWidth.current + d)))}
      />
      <div className="wb-svc-detail">
        {selected?.kind === 'container' ? (
          <ContainerDetail key={selected.c.id} c={selected.c} projectId={projectId} projects={projects ?? []} onSelectImage={(id) => setSel(rows.find((r) => r.kind === 'image' && r.img.id === id)?.key ?? sel)} />
        ) : selected?.kind === 'compose' ? (
          <ComposeDetail node={selected.node} projects={projects ?? []} onSelect={(id) => setSel(`c:${id}`)} />
        ) : selected?.kind === 'image' ? (
          <ImageDetail key={selected.key} img={selected.img} onSelectContainer={(name) => setSel(rows.find((r) => r.kind === 'container' && r.c.name === name)?.key ?? sel)} />
        ) : selected?.kind === 'header' && selected.section === 'images' ? (
          <ImagesSummary images={images.data?.images ?? []} />
        ) : (
          <EmptyState icon={Container} title="Select a container, compose project or image">
            {current ? `${current.name}’s containers are listed first.` : null} Double-click a container (or press Enter) for its log.
          </EmptyState>
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- tree rows

function containerMenu(c: DockerContainer, projectId: string | null): MenuEntry[] {
  const running = isRunning(c)
  return [
    { label: 'Show Log', icon: ScrollText, run: () => void showLogs(c.id, projectId) },
    { label: 'Open Shell', icon: SquareTerminal, disabled: c.state !== 'running', run: () => void openShell(c.id, projectId) },
    'separator',
    { label: 'Start', icon: Play, disabled: running, run: () => void containerAction(c.id, c.name, 'start') },
    { label: 'Stop', icon: Square, disabled: !running, run: () => void containerAction(c.id, c.name, 'stop') },
    { label: 'Restart', icon: RotateCw, disabled: !running, run: () => void containerAction(c.id, c.name, 'restart') },
    c.state === 'paused'
      ? { label: 'Unpause', icon: Play, run: () => void containerAction(c.id, c.name, 'unpause') }
      : { label: 'Pause', icon: Pause, disabled: c.state !== 'running', run: () => void containerAction(c.id, c.name, 'pause') },
    { label: 'Kill', icon: Skull, disabled: !running, run: () => void containerAction(c.id, c.name, 'kill') },
    'separator',
    { label: 'Copy Container ID', icon: Copy, run: () => void copy(c.id) },
    { label: 'Remove…', icon: Trash2, danger: true, run: () => void confirmRemoveContainer(c) },
  ]
}

function TreeRow({ row, selected, open, projectId, onSelect, onToggle }: { row: Row; selected: boolean; open: boolean; projectId: string | null; onSelect: () => void; onToggle: () => void }) {
  const cls = `wb-list-row wb-svc-row${selected ? ' selected' : ''}`
  const chevron = (
    <span
      className="wb-svc-chev"
      onClick={(e) => {
        e.stopPropagation()
        onToggle()
      }}
    >
      {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
    </span>
  )
  if (row.kind === 'header') {
    return (
      <div className={`${cls} wb-svc-head`} data-key={row.key} role="treeitem" aria-expanded={open} aria-selected={selected} onClick={onSelect} onDoubleClick={onToggle}>
        {chevron}
        {row.section === 'containers' ? <Container size={13} /> : <Layers size={13} />}
        <span className="wb-svc-name">{row.section === 'containers' ? 'Containers' : 'Images'}</span>
        <span className="wb-subtle wb-small">{row.count}</span>
        {row.section === 'images' && (
          <>
            <span className="wb-grow" />
            <IconButton icon={Eraser} size="small" label="Remove dangling images…" onClick={(e) => (e.stopPropagation(), void confirmPrune())} />
          </>
        )}
      </div>
    )
  }
  if (row.kind === 'compose') {
    const n = row.node
    const up = n.containers.filter(isRunning).length
    const tone = up === 0 ? 'muted' : up === n.containers.length ? 'success' : 'warning'
    return (
      <div
        className={`${cls} wb-svc-depth1`}
        data-key={row.key}
        role="treeitem"
        aria-expanded={open}
        aria-selected={selected}
        title={n.workingDir}
        onClick={onSelect}
        onDoubleClick={onToggle}
        onContextMenu={(e) =>
          showMenu(e, [
            { label: 'Start', icon: Play, run: () => void composeAction(n.project, 'start') },
            { label: 'Stop', icon: Square, disabled: up === 0, run: () => void composeAction(n.project, 'stop') },
            { label: 'Restart', icon: RotateCw, disabled: up === 0, run: () => void composeAction(n.project, 'restart') },
            'separator',
            { label: 'Down…', icon: Trash2, danger: true, run: () => void confirmDown(n.project) },
          ])
        }
      >
        {chevron}
        <StatusDot tone={tone} />
        <Boxes size={13} />
        <span className="wb-svc-name wb-ellipsis">{n.project}</span>
        {n.projectId && n.projectId === projectId && <Badge tone="accent">this project</Badge>}
        <span className="wb-grow" />
        <span className="wb-subtle wb-small">
          {up}/{n.containers.length}
        </span>
      </div>
    )
  }
  if (row.kind === 'container') {
    const c = row.c
    return (
      <div
        className={`${cls} wb-svc-depth${row.depth}`}
        data-key={row.key}
        role="treeitem"
        aria-selected={selected}
        title={`${c.name}\n${c.image}\n${c.status}`}
        onClick={onSelect}
        onDoubleClick={() => void showLogs(c.id, projectId)}
        onContextMenu={(e) => {
          onSelect()
          showMenu(e, containerMenu(c, projectId))
        }}
      >
        <StatusDot tone={containerTone(c)} pulse={c.state === 'restarting'} />
        <Box size={13} />
        <span className="wb-svc-name wb-ellipsis">{row.label}</span>
        {row.depth === 1 && c.devcontainer && <Badge>dev container</Badge>}
        {row.depth === 1 && c.projectId && c.projectId === projectId && <Badge tone="accent">this project</Badge>}
        <span className="wb-subtle wb-small wb-ellipsis wb-svc-image">{c.image}</span>
        <span className="wb-grow" />
        <span className="wb-subtle wb-xs wb-svc-status">{c.status}</span>
      </div>
    )
  }
  const img = row.img
  return (
    <div
      className={`${cls} wb-svc-depth1`}
      data-key={row.key}
      role="treeitem"
      aria-selected={selected}
      title={`${imageName(img)}\n${img.id}`}
      onClick={onSelect}
      onContextMenu={(e) => {
        onSelect()
        showMenu(e, [
          { label: 'Copy Image ID', icon: Copy, run: () => void copy(img.id) },
          { label: 'Remove…', icon: Trash2, danger: true, disabled: img.containers.length > 0 && img.repository === '<none>', run: () => void confirmRemoveImage(img) },
        ])
      }}
    >
      <Layers size={13} className={img.containers.length ? undefined : 'wb-subtle'} />
      <span className={`wb-svc-name wb-ellipsis${img.repository === '<none>' ? ' wb-subtle' : ''}`}>{imageName(img)}</span>
      <span className="wb-grow" />
      {img.containers.length > 0 && <span className="wb-subtle wb-xs wb-svc-inuse">in use</span>}
      <span className="wb-subtle wb-xs wb-svc-size">{img.size}</span>
    </div>
  )
}

// ---------------------------------------------------------------- actions

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast('success', 'Copied')
  } catch {
    toast('warning', 'The browser refused the clipboard')
  }
}

async function confirmRemoveContainer(c: DockerContainer) {
  const running = isRunning(c)
  const ok = await confirmDialog({
    title: `Remove ${c.name}?`,
    message: `${running ? 'It is running: it is killed first (docker rm --force). ' : ''}Anything written inside the container that is not in a volume or bind mount is lost. Its image and named volumes stay.`,
    confirmLabel: 'Remove',
    danger: true,
  })
  if (ok) await containerAction(c.id, c.name, 'remove', running)
}

async function confirmDown(project: string) {
  const ok = await confirmDialog({
    title: `Take ${project} down?`,
    message: `docker compose down: its containers and networks are removed. Named volumes and images stay; bring it up again from its compose file.`,
    confirmLabel: 'Down',
    danger: true,
  })
  if (ok) await composeAction(project, 'down')
}

async function confirmRemoveImage(img: DockerImage) {
  const ref = imageRef(img)
  const ok = await confirmDialog({
    title: `Remove ${imageName(img)}?`,
    message:
      ref === img.id
        ? 'docker rmi: the image is deleted. Docker refuses while a container uses it.'
        : `docker rmi ${ref}: the tag is removed, and the image with it when no other tag names it. Docker refuses while a container uses it.`,
    confirmLabel: 'Remove',
    danger: true,
  })
  if (ok) await removeImage(ref)
}

async function confirmPrune() {
  const ok = await confirmDialog({
    title: 'Remove dangling images?',
    message: 'docker image prune: images without a tag that no container uses are deleted. Tagged images stay.',
    confirmLabel: 'Remove',
    danger: true,
  })
  if (ok) await pruneImages()
}

// ---------------------------------------------------------------- details

function KV({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="wb-dc-kv">
      <span className="k">{k}</span>
      <span className="v">{children}</span>
    </div>
  )
}

function When({ at }: { at: string | null | undefined }) {
  const t = dockerTime(at)
  return t ? <TimeAgo time={t} /> : <span className="wb-subtle">—</span>
}

function Ports({ ports }: { ports: PortMap[] }) {
  if (!ports.length) return <span className="wb-subtle">none</span>
  return (
    <>
      {ports.map((p) => {
        const url = portUrl(p, location.hostname)
        const label = portLabel(p)
        return url ? (
          <a key={label} className="wb-svc-port" href={url} target="_blank" rel="noreferrer noopener" title={`Open ${url}`}>
            {label} <ExternalLink size={11} />
          </a>
        ) : (
          <code key={label} className="wb-svc-port" title={p.hostPort ? 'Bound to this computer’s loopback: open it from a browser here' : 'Not published to the host'}>
            {label}
          </code>
        )
      })}
    </>
  )
}

function Table({ rows, empty }: { rows: [string, string][]; empty: string }) {
  if (!rows.length) return <EmptyState title={empty} />
  return (
    <table className="wb-svc-table">
      <tbody>
        {rows.map(([k, v], i) => (
          <tr key={`${k}:${i}`}>
            <th>{k}</th>
            <td>{v}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function Inspect({ id, value }: { id: string; value: unknown }) {
  const theme = useUi((s) => s.prefs.theme)
  const fontSize = useUi((s) => s.prefs.editorFontSize)
  const text = useMemo(() => JSON.stringify(value, null, 2), [value])
  return (
    <div className="wb-svc-inspect">
      <MonacoEditor
        theme={theme === 'dark' ? 'workbench-dark' : 'workbench-light'}
        language="json"
        value={text}
        path={`inmemory://docker-inspect/${id.replace(/[^A-Za-z0-9]/g, '')}.json`}
        options={{ readOnly: true, minimap: { enabled: false }, fontSize, scrollBeyondLastLine: false, automaticLayout: true, glyphMargin: false, folding: true }}
      />
    </div>
  )
}

/** A compose file: opens in the editor when it is inside a Workbench project. */
function ComposeFile({ path, projects }: { path: string; projects: ProjectSummary[] }) {
  for (const p of projects) {
    const rel = relativeTo(p.rootAbs, path)
    if (rel) {
      return (
        <button
          type="button"
          className="wb-svc-link wb-svc-file"
          title={`Open ${rel} (${p.name})`}
          onClick={() => openPanel({ kind: 'editor', id: `editor:${p.id}:${rel}`, params: { projectId: p.id, path: rel } })}
        >
          <FileCode size={12} /> {rel}
        </button>
      )
    }
  }
  return <code className="wb-svc-file">{path}</code>
}

function ComposeKV({ compose, projects }: { compose: ComposeInfo; projects: ProjectSummary[] }) {
  return (
    <>
      <KV k="Compose">
        {compose.project}
        {compose.service && <span className="wb-subtle">· service {compose.service}</span>}
      </KV>
      {compose.configFiles.length > 0 && (
        <KV k="Files">
          {compose.configFiles.map((f) => (
            <ComposeFile key={f} path={f} projects={projects} />
          ))}
        </KV>
      )}
    </>
  )
}

type DetailTab = 'info' | 'env' | 'labels' | 'inspect'

function ContainerDetail({ c, projectId, projects, onSelectImage }: { c: DockerContainer; projectId: string | null; projects: ProjectSummary[]; onSelectImage: (id: string) => void }) {
  const q = useContainer(c.id)
  const d = q.data
  const [tab, setTab] = useState<DetailTab>('info')
  const [busy, setBusy] = useState<string | null>(null)
  const running = isRunning(c)
  const act = async (a: 'start' | 'stop' | 'restart' | 'pause' | 'unpause') => {
    setBusy(a)
    await containerAction(c.id, c.name, a)
    setBusy(null)
  }
  const owner = projects.find((p) => p.id === (d?.projectId ?? c.projectId))
  return (
    <div className="wb-fill wb-svc-pane">
      <div className="wb-svc-head-bar">
        <StatusDot tone={containerTone(c)} pulse={c.state === 'restarting'} />
        <b className="wb-ellipsis">{c.name}</b>
        <span className="wb-subtle wb-small wb-ellipsis">{c.status}</span>
        <span className="wb-grow" />
        {running ? (
          <>
            <Button size="small" icon={Square} loading={busy === 'stop'} onClick={() => void act('stop')}>
              Stop
            </Button>
            <Button size="small" icon={RotateCw} loading={busy === 'restart'} onClick={() => void act('restart')}>
              Restart
            </Button>
            {c.state === 'paused' ? (
              <Button size="small" icon={Play} loading={busy === 'unpause'} onClick={() => void act('unpause')}>
                Unpause
              </Button>
            ) : (
              <Button size="small" icon={Pause} loading={busy === 'pause'} onClick={() => void act('pause')}>
                Pause
              </Button>
            )}
          </>
        ) : (
          <Button size="small" variant="primary" icon={Play} loading={busy === 'start'} onClick={() => void act('start')}>
            Start
          </Button>
        )}
        <Button size="small" icon={ScrollText} onClick={() => void showLogs(c.id, projectId)}>
          Log
        </Button>
        <Button size="small" icon={SquareTerminal} disabled={c.state !== 'running'} onClick={() => void openShell(c.id, projectId)}>
          Shell
        </Button>
        <IconButton icon={Trash2} size="small" label="Remove…" onClick={() => void confirmRemoveContainer(c)} />
      </div>
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { id: 'info', label: 'Info' },
          { id: 'env', label: `Environment${d ? ` (${d.env.length})` : ''}` },
          { id: 'labels', label: `Labels${d ? ` (${d.labels.length})` : ''}` },
          { id: 'inspect', label: 'Inspect' },
        ]}
      />
      {q.error ? (
        <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
      ) : !d ? (
        <Loading label="Inspecting…" />
      ) : tab === 'info' ? (
        <div className="wb-scroll wb-svc-info">
          <KV k="ID">
            <code>{shortId(d.id)}</code>
            <IconButton icon={Copy} size="small" label="Copy the full ID" onClick={() => void copy(d.id)} />
          </KV>
          <KV k="Image">
            <button type="button" className="wb-svc-link" title={d.imageId} onClick={() => onSelectImage(d.imageId)}>
              {d.image}
            </button>
          </KV>
          {d.command && (
            <KV k="Command">
              <code className="wb-svc-cmd">{d.command}</code>
            </KV>
          )}
          <KV k="Created">
            <When at={d.createdAt} />
          </KV>
          {d.running ? (
            <KV k="Started">
              <When at={d.startedAt} />
            </KV>
          ) : (
            d.finishedAt && (
              <KV k="Finished">
                <When at={d.finishedAt} />
                {d.exitCode !== null && <span className={d.exitCode === 0 ? 'wb-subtle' : 'wb-danger'}>exit code {d.exitCode}</span>}
              </KV>
            )
          )}
          {d.error && (
            <KV k="Error">
              <span className="wb-danger">{d.error}</span>
            </KV>
          )}
          {d.health && (
            <KV k="Health">
              <Badge tone={d.health === 'healthy' ? 'success' : d.health === 'unhealthy' ? 'danger' : 'warning'}>{d.health}</Badge>
            </KV>
          )}
          {d.restartPolicy && (
            <KV k="Restart">
              {d.restartPolicy}
              {d.restartCount > 0 && <span className="wb-subtle">· restarted {d.restartCount}×</span>}
            </KV>
          )}
          <KV k="Ports">
            <Ports ports={d.ports} />
          </KV>
          {d.networks.length > 0 && (
            <KV k="Networks">
              {d.networks.map((n) => (
                <span key={n.name} title={n.aliases.length ? `aliases: ${n.aliases.join(', ')}` : undefined}>
                  {n.name}
                  {n.ip && <span className="wb-subtle"> {n.ip}</span>}
                </span>
              ))}
            </KV>
          )}
          {!d.networks.length && d.networkMode && <KV k="Network">{d.networkMode}</KV>}
          {d.mounts.length > 0 && (
            <KV k="Mounts">
              <span className="wb-svc-mounts">
                {d.mounts.map((m) => (
                  <span key={m.destination} className="wb-ellipsis" title={`${m.type} ${m.source} → ${m.destination}${m.rw ? '' : ' (read-only)'}`}>
                    <span className="wb-subtle">{m.type}</span> {m.source || '(anonymous)'} → {m.destination}
                    {!m.rw && <span className="wb-subtle"> ro</span>}
                  </span>
                ))}
              </span>
            </KV>
          )}
          {d.workingDir && (
            <KV k="Workdir">
              <code>{d.workingDir}</code>
            </KV>
          )}
          {d.user && <KV k="User">{d.user}</KV>}
          {d.compose && <ComposeKV compose={d.compose} projects={projects} />}
          {d.devcontainer && (
            <KV k="Dev container">
              <code>{d.devcontainer}</code>
              {owner && (
                <Button size="small" icon={Container} onClick={() => openDevcontainerPanel(owner.id)}>
                  Dev container panel
                </Button>
              )}
            </KV>
          )}
          {owner && !d.devcontainer && <KV k="Project">{owner.name}</KV>}
        </div>
      ) : tab === 'env' ? (
        <div className="wb-scroll wb-svc-info">
          <Table rows={d.env} empty="No environment variables" />
          <p className="wb-small wb-subtle">Values of secret-looking names (passwords, tokens, keys) and passwords in URLs are shown as ••••.</p>
        </div>
      ) : tab === 'labels' ? (
        <div className="wb-scroll wb-svc-info">
          <Table rows={d.labels} empty="No labels" />
        </div>
      ) : (
        <Inspect id={d.id} value={d.inspect} />
      )}
    </div>
  )
}

function ComposeDetail({ node, projects, onSelect }: { node: Extract<TreeNode, { kind: 'compose' }>; projects: ProjectSummary[]; onSelect: (id: string) => void }) {
  const [busy, setBusy] = useState<string | null>(null)
  const up = node.containers.filter(isRunning).length
  const act = async (a: 'start' | 'stop' | 'restart') => {
    setBusy(a)
    await composeAction(node.project, a)
    setBusy(null)
  }
  const files = node.containers[0]?.compose?.configFiles ?? []
  const owner = projects.find((p) => p.id === node.projectId)
  return (
    <div className="wb-fill wb-svc-pane">
      <div className="wb-svc-head-bar">
        <StatusDot tone={up === 0 ? 'muted' : up === node.containers.length ? 'success' : 'warning'} />
        <b className="wb-ellipsis">{node.project}</b>
        <span className="wb-subtle wb-small">
          compose · {up} of {node.containers.length} running
        </span>
        <span className="wb-grow" />
        <Button size="small" variant={up < node.containers.length ? 'primary' : 'default'} icon={Play} loading={busy === 'start'} onClick={() => void act('start')}>
          Start
        </Button>
        <Button size="small" icon={Square} disabled={up === 0} loading={busy === 'stop'} onClick={() => void act('stop')}>
          Stop
        </Button>
        <Button size="small" icon={RotateCw} disabled={up === 0} loading={busy === 'restart'} onClick={() => void act('restart')}>
          Restart
        </Button>
        <Button size="small" variant="danger" icon={Trash2} onClick={() => void confirmDown(node.project)}>
          Down…
        </Button>
      </div>
      <div className="wb-scroll wb-svc-info">
        <KV k="Folder">
          <code>{node.workingDir}</code>
        </KV>
        {files.length > 0 && (
          <KV k="Files">
            {files.map((f) => (
              <ComposeFile key={f} path={f} projects={projects} />
            ))}
          </KV>
        )}
        {owner && <KV k="Project">{owner.name}</KV>}
        <div className="wb-svc-services">
          {node.containers.map((c) => (
            <div key={c.id} className="wb-list-row wb-svc-row" onClick={() => onSelect(c.id)} onDoubleClick={() => void showLogs(c.id, node.projectId ?? null)} title="Double-click for its log">
              <StatusDot tone={containerTone(c)} />
              <span className="wb-svc-name">{serviceLabel(c, node.containers)}</span>
              <span className="wb-subtle wb-small wb-ellipsis">{c.image}</span>
              <span className="wb-grow" />
              {c.ports.filter((p) => p.hostPort).map((p) => (
                <code key={portLabel(p)} className="wb-subtle wb-xs">
                  {portLabel(p)}
                </code>
              ))}
              <span className="wb-subtle wb-xs">{c.status}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

function ImageDetail({ img, onSelectContainer }: { img: DockerImage; onSelectContainer: (name: string) => void }) {
  const q = useImage(img.id)
  const d = q.data
  const [tab, setTab] = useState<DetailTab>('info')
  return (
    <div className="wb-fill wb-svc-pane">
      <div className="wb-svc-head-bar">
        <Layers size={14} />
        <b className="wb-ellipsis">{imageName(img)}</b>
        <span className="wb-subtle wb-small">{img.size}</span>
        <span className="wb-grow" />
        <Button size="small" icon={Trash2} onClick={() => void confirmRemoveImage(img)} title={img.containers.length ? `Used by ${img.containers.join(', ')}` : undefined}>
          Remove…
        </Button>
      </div>
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { id: 'info', label: 'Info' },
          { id: 'env', label: `Environment${d ? ` (${d.env.length})` : ''}` },
          { id: 'labels', label: `Labels${d ? ` (${d.labels.length})` : ''}` },
          { id: 'inspect', label: 'Inspect' },
        ]}
      />
      {q.error ? (
        <ErrorBox error={q.error} onRetry={() => void q.refetch()} />
      ) : !d ? (
        <Loading label="Inspecting…" />
      ) : tab === 'info' ? (
        <div className="wb-scroll wb-svc-info">
          <KV k="ID">
            <code>{shortId(d.id)}</code>
            <IconButton icon={Copy} size="small" label="Copy the full ID" onClick={() => void copy(d.id)} />
          </KV>
          {d.tags.length > 0 && <KV k="Tags">{d.tags.map((t) => <code key={t}>{t}</code>)}</KV>}
          <KV k="Created">
            <When at={d.createdAt} />
          </KV>
          <KV k="Size">
            {/* The list's size is what the image takes on disk; inspect's is its content. */}
            <span title={`${formatBytes(d.size)} of content`}>{img.size}</span> <span className="wb-subtle">· {d.layers} layers</span>
          </KV>
          <KV k="Platform">
            {d.os}/{d.architecture}
          </KV>
          {d.entrypoint && (
            <KV k="Entrypoint">
              <code className="wb-svc-cmd">{d.entrypoint}</code>
            </KV>
          )}
          {d.cmd && (
            <KV k="Cmd">
              <code className="wb-svc-cmd">{d.cmd}</code>
            </KV>
          )}
          {d.workingDir && (
            <KV k="Workdir">
              <code>{d.workingDir}</code>
            </KV>
          )}
          {d.user && <KV k="User">{d.user}</KV>}
          {d.exposedPorts.length > 0 && <KV k="Exposes">{d.exposedPorts.join(', ')}</KV>}
          <KV k="Used by">
            {img.containers.length ? (
              img.containers.map((n) => (
                <button key={n} type="button" className="wb-svc-link" onClick={() => onSelectContainer(n)}>
                  {n}
                </button>
              ))
            ) : (
              <span className="wb-subtle">no container</span>
            )}
          </KV>
          {d.digests.length > 0 && (
            <KV k="Digests">
              <span className="wb-svc-mounts">
                {d.digests.map((x) => (
                  <code key={x} className="wb-ellipsis">
                    {x}
                  </code>
                ))}
              </span>
            </KV>
          )}
        </div>
      ) : tab === 'env' ? (
        <div className="wb-scroll wb-svc-info">
          <Table rows={d.env} empty="No environment variables" />
        </div>
      ) : tab === 'labels' ? (
        <div className="wb-scroll wb-svc-info">
          <Table rows={d.labels} empty="No labels" />
        </div>
      ) : (
        <Inspect id={d.id} value={d.inspect} />
      )}
    </div>
  )
}

function ImagesSummary({ images }: { images: DockerImage[] }) {
  const dangling = images.filter((i) => i.repository === '<none>')
  const unused = images.filter((i) => !i.containers.length)
  return (
    <div className="wb-fill wb-svc-pane">
      <div className="wb-svc-head-bar">
        <Layers size={14} />
        <b>Images</b>
        <span className="wb-subtle wb-small">
          {images.length} · {unused.length} unused · {dangling.length} dangling
        </span>
        <span className="wb-grow" />
        <Button size="small" icon={Eraser} disabled={!dangling.length} onClick={() => void confirmPrune()}>
          Remove Dangling…
        </Button>
      </div>
      <EmptyState icon={Layers} title="Select an image">
        Unused images are dimmed. Removing a tag keeps the image while another tag names it.
      </EmptyState>
    </div>
  )
}
