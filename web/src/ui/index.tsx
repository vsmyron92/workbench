// Shared UI primitives. Features compose these rather than styling from scratch,
// so every tool window looks like one application.

import {
  Component,
  forwardRef,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type ComponentProps,
  type ComponentType,
  type CSSProperties,
  type ErrorInfo,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from 'react'
import { createPortal } from 'react-dom'
import { AlertTriangle, ChevronDown, ChevronRight, MonitorX, Settings2 } from 'lucide-react'
import { ApiError } from '@/api/client'
import { osLabel, useHealth } from '@/api/health'
import { isMobileShell, openSettings } from '@/shell/actions'
import type { AnsiLog as AnsiLogT } from './AnsiLog'
import type { Markdown as MarkdownT } from './Markdown'
import './ui.css'

type Icon = ComponentType<{ size?: number; className?: string }>

// ---------------------------------------------------------------- buttons

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'default' | 'primary' | 'danger' | 'ghost'
  size?: 'default' | 'small'
  icon?: Icon
  loading?: boolean
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'default', size = 'default', icon: I, loading, children, className, disabled, ...rest },
  ref,
) {
  const cls = ['wb-btn', variant !== 'default' && variant, size === 'small' && 'small', className].filter(Boolean).join(' ')
  return (
    <button ref={ref} type="button" className={cls} disabled={disabled || loading} {...rest}>
      {loading ? <Spinner /> : I ? <I size={size === 'small' ? 13 : 15} /> : null}
      {children}
    </button>
  )
})

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  icon: Icon
  /** Tooltip and accessible label. */
  label: string
  active?: boolean
  size?: 'default' | 'small'
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { icon: I, label, active, size = 'default', className, ...rest },
  ref,
) {
  const cls = ['wb-icon-btn', active && 'active', size === 'small' && 'small', className].filter(Boolean).join(' ')
  return (
    <button ref={ref} type="button" className={cls} title={label} aria-label={label} {...rest}>
      <I size={size === 'small' ? 14 : 16} />
    </button>
  )
})

// ---------------------------------------------------------------- inputs

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement> & { small?: boolean }>(
  function Input({ className, small, ...rest }, ref) {
    return <input ref={ref} className={['wb-input', small && 'small', className].filter(Boolean).join(' ')} {...rest} />
  },
)

export const TextArea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(function TextArea(
  { className, ...rest },
  ref,
) {
  return <textarea ref={ref} className={['wb-textarea', className].filter(Boolean).join(' ')} {...rest} />
})

export function Select({ className, children, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select className={['wb-select', className].filter(Boolean).join(' ')} {...rest}>
      {children}
    </select>
  )
}

export function Checkbox({
  checked,
  onChange,
  children,
  disabled,
}: {
  checked: boolean
  onChange: (v: boolean) => void
  children?: ReactNode
  disabled?: boolean
}) {
  return (
    <label className="wb-checkbox">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      {children}
    </label>
  )
}

export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="wb-field">
      <label>{label}</label>
      {children}
      {hint && <div className="hint">{hint}</div>}
    </div>
  )
}

// ---------------------------------------------------------------- feedback

export function Spinner({ size = 14 }: { size?: number }) {
  return <span className="wb-spinner" style={{ width: size, height: size }} aria-label="Loading" />
}

export function Loading({ label = 'Loading…' }: { label?: string }) {
  return (
    <div className="wb-empty">
      <Spinner size={18} />
      <span>{label}</span>
    </div>
  )
}

/**
 * Contains a render error to the subtree: `fallback` is shown in its place (and the error
 * logged) instead of React unmounting the whole app. A new `key` starts it over.
 */
export class ErrorBoundary extends Component<{ children: ReactNode; fallback: (error: unknown, reset: () => void) => ReactNode; label?: string }, { error: unknown }> {
  state = { error: null as unknown }
  static getDerivedStateFromError(error: unknown) {
    return { error: error ?? new Error('render failed') }
  }
  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error(`[workbench] ${this.props.label ?? 'view'} failed to render`, error, info.componentStack)
  }
  render() {
    if (this.state.error) return this.props.fallback(this.state.error, () => this.setState({ error: null }))
    return this.props.children
  }
}

/** Around a dialog host: a dialog that throws while rendering becomes an error dialog. */
export function DialogBoundary({ title, onClose, children }: { title: ReactNode; onClose: () => void; children: ReactNode }) {
  return (
    <ErrorBoundary
      label="dialog"
      fallback={(error) => (
        <Modal title={title} onClose={onClose} footer={<Button onClick={onClose}>Close</Button>}>
          <ErrorBox error={error} />
        </Modal>
      )}
    >
      {children}
    </ErrorBoundary>
  )
}

export function EmptyState({
  icon: I,
  title,
  children,
  action,
}: {
  icon?: Icon
  title: string
  children?: ReactNode
  action?: ReactNode
}) {
  return (
    <div className="wb-empty">
      {I && <I size={28} className="icon" />}
      <div className="title">{title}</div>
      {children && <div className="wb-small">{children}</div>}
      {action}
    </div>
  )
}

/**
 * Shows an error; `not_configured` errors get a setup-flavoured box with a link to
 * Settings (`settingsSection`, default Integrations; not on a phone, which has no
 * Settings panel). Edits to config.toml apply live, so Retry works after either.
 * `unsupported_platform` errors (the server's OS leaves the feature out) get the same
 * box with the reason, and neither Settings nor Retry, which cannot change that.
 */
export function ErrorBox({ error, onRetry, settingsSection = 'integrations' }: { error: unknown; onRetry?: () => void; settingsSection?: string }) {
  const os = osLabel(useHealth()?.os)
  const kind = errorKind(error)
  const setup = kind === 'setup'
  const msg = error instanceof Error ? error.message : String(error)
  const settingsLink = setup && !isMobileShell()
  const retry = kind === 'unsupported' ? undefined : onRetry
  return (
    <div className={kind === 'error' ? 'wb-error' : 'wb-error setup'}>
      <div className="wb-row" style={{ alignItems: 'flex-start' }}>
        {kind === 'unsupported' ? <MonitorX size={16} /> : setup ? <Settings2 size={16} /> : <AlertTriangle size={16} className="wb-danger" />}
        <div className="wb-grow">
          <div style={{ fontWeight: 600, marginBottom: 2 }}>
            {kind === 'unsupported' ? `Not available on ${os ?? 'this system'}` : setup ? 'Not set up yet' : 'Something went wrong'}
          </div>
          <div className="wb-small">{msg}</div>
        </div>
      </div>
      {(retry || settingsLink) && (
        <div className="wb-row" style={{ marginTop: 8, gap: 6 }}>
          {settingsLink && (
            <Button size="small" icon={Settings2} onClick={() => openSettings(settingsSection)}>
              Open Settings
            </Button>
          )}
          {retry && (
            <Button size="small" onClick={retry}>
              Retry
            </Button>
          )}
        </div>
      )}
    </div>
  )
}

/** How `ErrorBox` shows an error: setup help, a feature this OS leaves out, or a failure. */
export function errorKind(error: unknown): 'setup' | 'unsupported' | 'error' {
  if (!(error instanceof ApiError)) return 'error'
  return error.notConfigured ? 'setup' : error.unsupported ? 'unsupported' : 'error'
}

export function Badge({ tone, children, title }: { tone?: 'accent' | 'success' | 'warning' | 'danger'; children: ReactNode; title?: string }) {
  return (
    <span className={['wb-badge', tone].filter(Boolean).join(' ')} title={title}>
      {children}
    </span>
  )
}

export function StatusDot({
  tone,
  pulse,
  title,
}: {
  tone?: 'success' | 'warning' | 'danger' | 'accent' | 'muted'
  pulse?: boolean
  title?: string
}) {
  return <span className={['wb-dot', tone !== 'muted' && tone, pulse && 'pulse'].filter(Boolean).join(' ')} title={title} />
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="wb-kbd">{children}</kbd>
}

// ---------------------------------------------------------------- layout

export function Toolbar({ title, children, style }: { title?: ReactNode; children?: ReactNode; style?: CSSProperties }) {
  return (
    <div className="wb-toolbar" style={style}>
      {title && <span className="title">{title}</span>}
      {children}
    </div>
  )
}

export function Spacer() {
  return <span className="spacer" style={{ flex: 1 }} />
}

export function Tabs<T extends string>({
  tabs,
  value,
  onChange,
}: {
  tabs: { id: T; label: ReactNode; badge?: ReactNode }[]
  value: T
  onChange: (id: T) => void
}) {
  return (
    <div className="wb-tabs" role="tablist">
      {tabs.map((t) => (
        <button key={t.id} role="tab" aria-selected={t.id === value} className={t.id === value ? 'wb-tab active' : 'wb-tab'} onClick={() => onChange(t.id)}>
          {t.label}
          {t.badge}
        </button>
      ))}
    </div>
  )
}

/** Collapsible section with an uppercase header (tool windows). */
export function Section({
  title,
  defaultOpen = true,
  actions,
  count,
  children,
}: {
  title: string
  defaultOpen?: boolean
  actions?: ReactNode
  count?: number
  children: ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <div>
      <div className="wb-section-header" onClick={() => setOpen(!open)}>
        <button
          type="button"
          className="wb-section-toggle"
          aria-expanded={open}
          onClick={(e) => {
            e.stopPropagation()
            setOpen(!open)
          }}
        >
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          <span>{title}</span>
          {count !== undefined && <span className="wb-subtle">{count}</span>}
        </button>
        <span style={{ flex: 1 }} />
        <span onClick={(e) => e.stopPropagation()} className="wb-row">
          {actions}
        </span>
      </div>
      {open && children}
    </div>
  )
}

/**
 * Drag handle between two panes. `onResize` gets the pointer delta in px since the
 * drag started (positive = right/down); the caller clamps and applies it.
 */
export function Splitter({
  direction,
  onResizeStart,
  onResize,
}: {
  direction: 'v' | 'h'
  onResizeStart?: () => void
  onResize: (delta: number) => void
}) {
  const [dragging, setDragging] = useState(false)
  const onPointerDown = (e: React.PointerEvent) => {
    e.preventDefault()
    const start = direction === 'v' ? e.clientX : e.clientY
    onResizeStart?.()
    setDragging(true)
    const move = (ev: PointerEvent) => onResize((direction === 'v' ? ev.clientX : ev.clientY) - start)
    const up = () => {
      setDragging(false)
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      document.body.style.cursor = ''
    }
    document.body.style.cursor = direction === 'v' ? 'col-resize' : 'row-resize'
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  return <div className={`wb-splitter ${direction}${dragging ? ' dragging' : ''}`} onPointerDown={onPointerDown} />
}

// ---------------------------------------------------------------- menus

export interface MenuItem {
  label: string
  icon?: Icon
  shortcut?: string
  danger?: boolean
  disabled?: boolean
  run: () => void
}
export type MenuEntry = MenuItem | 'separator'

interface MenuState {
  x: number
  y: number
  items: MenuEntry[]
}

let setMenuGlobal: ((m: MenuState | null) => void) | null = null

/** Open a context menu at the pointer: `onContextMenu={(e) => showMenu(e, [...])}`. */
export function showMenu(e: { clientX: number; clientY: number; preventDefault?: () => void }, items: MenuEntry[]) {
  e.preventDefault?.()
  setMenuGlobal?.({ x: e.clientX, y: e.clientY, items })
}

/** Open a menu below an element (dropdown buttons). */
export function showMenuAt(el: HTMLElement, items: MenuEntry[]) {
  const r = el.getBoundingClientRect()
  setMenuGlobal?.({ x: r.left, y: r.bottom + 2, items })
}

/** Focusable elements inside `root`, in tab order. */
function focusables(root: HTMLElement): HTMLElement[] {
  const sel =
    'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]'
  return [...root.querySelectorAll<HTMLElement>(sel)].filter((el) => el.getClientRects().length > 0)
}

/** Mounted once by the shell. */
export function MenuHost() {
  const [menu, setMenu] = useState<MenuState | null>(null)
  const ref = useRef<HTMLDivElement>(null)
  const restoreFocus = useRef<HTMLElement | null>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  useEffect(() => {
    setMenuGlobal = (m) => {
      if (m && !ref.current) restoreFocus.current = document.activeElement as HTMLElement | null
      setMenu(m)
    }
    return () => {
      setMenuGlobal = null
    }
  }, [])
  useLayoutEffect(() => {
    if (!menu || !ref.current) return setPos(null)
    const r = ref.current.getBoundingClientRect()
    setPos({
      left: Math.max(4, Math.min(menu.x, window.innerWidth - r.width - 4)),
      top: Math.max(4, Math.min(menu.y, window.innerHeight - r.height - 4)),
    })
  }, [menu])
  // Keyboard users land on the first item.
  useEffect(() => {
    if (pos && ref.current) ref.current.querySelector<HTMLElement>('.wb-menu-item:not(:disabled)')?.focus({ preventScroll: true })
  }, [pos])
  const close = useCallback(() => {
    setMenu(null)
    const back = restoreFocus.current
    restoreFocus.current = null
    if (back?.isConnected) back.focus({ preventScroll: true })
  }, [])
  useEffect(() => {
    if (!menu) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || e.key === 'Tab') {
        if (e.key === 'Escape') e.preventDefault()
        return close()
      }
      if (!ref.current || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
      e.preventDefault()
      const items = [...ref.current.querySelectorAll<HTMLElement>('.wb-menu-item:not(:disabled)')]
      if (!items.length) return
      const i = items.indexOf(document.activeElement as HTMLElement)
      const next =
        e.key === 'Home' ? 0 : e.key === 'End' ? items.length - 1 : e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length
      items[next].focus({ preventScroll: true })
    }
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close()
    }
    window.addEventListener('keydown', onKey)
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('blur', close)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('blur', close)
    }
  }, [menu, close])
  if (!menu) return null
  return createPortal(
    <div ref={ref} className="wb-menu" role="menu" style={pos ?? { left: menu.x, top: menu.y, visibility: 'hidden' }}>
      {menu.items.map((it, i) =>
        it === 'separator' ? (
          <div key={i} className="wb-menu-sep" />
        ) : (
          <button
            key={i}
            role="menuitem"
            className={it.danger ? 'wb-menu-item danger' : 'wb-menu-item'}
            disabled={it.disabled}
            onClick={() => {
              close()
              it.run()
            }}
          >
            {it.icon ? <it.icon size={14} /> : <span style={{ width: 14 }} />}
            <span className="wb-ellipsis">{it.label}</span>
            {it.shortcut && <span className="shortcut">{it.shortcut}</span>}
          </button>
        ),
      )}
    </div>,
    document.body,
  )
}

// ---------------------------------------------------------------- modal

/** Open modals, innermost last: only the top one handles Escape and Tab. */
const modalStack: HTMLElement[] = []

/**
 * A dialog: labelled by its title, focus moves in (to an `autoFocus` field, else
 * the first control), Tab stays inside, and focus returns where it was on close.
 */
export function Modal({
  title,
  onClose,
  children,
  footer,
  wide,
}: {
  title: ReactNode
  onClose: () => void
  children: ReactNode
  footer?: ReactNode
  wide?: boolean
}) {
  const titleId = useId()
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const previous = document.activeElement as HTMLElement | null
    modalStack.push(el)
    if (!el.contains(document.activeElement)) (focusables(el.querySelector('.body') ?? el)[0] ?? focusables(el)[0] ?? el).focus({ preventScroll: true })
    return () => {
      modalStack.splice(modalStack.indexOf(el), 1)
      if (previous?.isConnected) previous.focus({ preventScroll: true })
    }
  }, [])
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = ref.current
      if (!el || modalStack[modalStack.length - 1] !== el) return
      if (e.key === 'Escape') return onClose()
      if (e.key !== 'Tab') return
      const items = focusables(el)
      if (!items.length) {
        e.preventDefault()
        return el.focus()
      }
      const first = items[0]
      const last = items[items.length - 1]
      const active = document.activeElement
      if (e.shiftKey && (active === first || !el.contains(active))) {
        e.preventDefault()
        last.focus()
      } else if (!e.shiftKey && (active === last || !el.contains(active))) {
        e.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return createPortal(
    <div className="wb-modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} tabIndex={-1} className={wide ? 'wb-modal wide' : 'wb-modal'} role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <header id={titleId}>{title}</header>
        {/* A dialog that cannot render its content shows why, and only Close: never its
            actions next to content that was not shown. */}
        <ErrorBoundary
          label="dialog"
          fallback={(error) => (
            <>
              <div className="body">
                <ErrorBox error={error} />
              </div>
              <footer>
                <Button onClick={onClose}>Close</Button>
              </footer>
            </>
          )}
        >
          <div className="body">{children}</div>
          {footer && <footer>{footer}</footer>}
        </ErrorBoundary>
      </div>
    </div>,
    document.body,
  )
}

// ---------------------------------------------------------------- time

export function timeAgo(ms: number | string | null | undefined): string {
  if (ms === null || ms === undefined) return ''
  const t = typeof ms === 'string' ? Date.parse(ms) : ms
  if (!Number.isFinite(t)) return ''
  const s = Math.round((Date.now() - t) / 1000)
  if (s < 45) return 'just now'
  const m = Math.round(s / 60)
  if (m < 60) return `${m} min ago`
  const h = Math.round(m / 60)
  if (h < 24) return `${h} h ago`
  const d = Math.round(h / 24)
  if (d < 30) return `${d} d ago`
  return new Date(t).toLocaleDateString()
}

/** Re-renders every `ms` so relative times stay fresh. */
export function TimeAgo({ time, ms = 30_000 }: { time: number | string | null | undefined; ms?: number }) {
  const [, tick] = useState(0)
  useEffect(() => {
    const t = window.setInterval(() => tick((x) => x + 1), ms)
    return () => window.clearInterval(t)
  }, [ms])
  const abs = time ? new Date(typeof time === 'string' ? Date.parse(time) : time).toLocaleString() : ''
  return <span title={abs}>{timeAgo(time)}</span>
}

export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return ''
  const s = Math.round(seconds)
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${s % 60}s`
  return `${Math.floor(m / 60)}h ${m % 60}m`
}

export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return ''
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`
}

export { BrandIcon, GitLabIcon, ConfluenceIcon, JiraIcon } from './brand'
export { MonacoEditor, MonacoDiffEditor } from './monaco'

// xterm (~650 KB) and the markdown/highlight stack load the first time a log or a
// markdown view is shown, not with the app.
const LazyAnsiLog = lazy(() => import('./AnsiLog'))
const LazyMarkdown = lazy(() => import('./Markdown'))

export function AnsiLog(props: ComponentProps<typeof AnsiLogT>) {
  return (
    <Suspense fallback={<div className="wb-log" />}>
      <LazyAnsiLog {...props} />
    </Suspense>
  )
}

export function Markdown(props: ComponentProps<typeof MarkdownT>) {
  return (
    <Suspense fallback={<div className="wb-prose" />}>
      <LazyMarkdown {...props} />
    </Suspense>
  )
}
