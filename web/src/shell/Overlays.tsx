// Toasts and the confirm/prompt dialogs driven by shell/actions.ts.

import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, CheckCircle2, Info, X, XCircle } from 'lucide-react'
import { Button, Input, Modal, TextArea } from '@/ui'
import { useDialogs, useToasts, type ToastLevel } from './actions'

const toastIcon: Record<ToastLevel, typeof Info> = {
  info: Info,
  success: CheckCircle2,
  warning: AlertTriangle,
  error: XCircle,
}

export function Toasts() {
  const { toasts, dismiss } = useToasts()
  return (
    <div className="wb-toasts" aria-live="polite">
      {toasts.map((t) => {
        const I = toastIcon[t.level]
        return (
          <div key={t.id} className={`wb-toast ${t.level}`}>
            <I size={16} className="icon" />
            <div className="wb-grow">
              <div className="msg">{t.message}</div>
              {t.detail && <div className="wb-small wb-muted wb-toast-detail">{t.detail}</div>}
              {t.code && <pre className="wb-toast-code">{t.code}</pre>}
              {(!!t.action || !!t.actions?.length) && (
                <div className="wb-toast-actions">
                  {t.action && (
                    <Button
                      size="small"
                      onClick={() => {
                        t.action!.run()
                        dismiss(t.id)
                      }}
                    >
                      {t.action.label}
                    </Button>
                  )}
                  {t.actions?.map((a) => (
                    <Button
                      key={a.label}
                      size="small"
                      variant={a.variant}
                      onClick={() => {
                        a.run()
                        dismiss(t.id)
                      }}
                    >
                      {a.label}
                    </Button>
                  ))}
                </div>
              )}
            </div>
            <button className="wb-icon-btn small" aria-label="Dismiss" onClick={() => dismiss(t.id)}>
              <X size={14} />
            </button>
          </div>
        )
      })}
    </div>
  )
}

export function Dialogs() {
  const { current, set } = useDialogs()
  const [value, setValue] = useState('')
  const inputRef = useRef<HTMLInputElement & HTMLTextAreaElement>(null)

  useEffect(() => {
    if (current?.kind === 'prompt') setValue(current.opts.initial ?? '')
    else setValue('')
    window.setTimeout(() => inputRef.current?.focus(), 0)
  }, [current])

  if (!current) return null

  if (current.kind === 'confirm') {
    const { opts, resolve } = current
    const close = (v: boolean) => {
      set(null)
      resolve(v)
    }
    const typedOk = !opts.typed || value === opts.typed
    return (
      <Modal
        title={opts.title}
        onClose={() => close(false)}
        footer={
          <>
            <Button onClick={() => close(false)}>Cancel</Button>
            <Button variant={opts.danger ? 'danger' : 'primary'} disabled={!typedOk} onClick={() => close(true)} autoFocus={!opts.typed}>
              {opts.confirmLabel ?? 'OK'}
            </Button>
          </>
        }
      >
        {opts.message && <div style={{ whiteSpace: 'pre-wrap' }}>{opts.message}</div>}
        {opts.typed && (
          <>
            <div className="wb-small wb-muted">
              Type <b className="mono">{opts.typed}</b> to confirm.
            </div>
            <Input
              ref={inputRef}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && typedOk && close(true)}
            />
          </>
        )}
      </Modal>
    )
  }

  const { opts, resolve } = current
  const close = (v: string | null) => {
    set(null)
    resolve(v)
  }
  return (
    <Modal
      title={opts.title}
      onClose={() => close(null)}
      footer={
        <>
          <Button onClick={() => close(null)}>Cancel</Button>
          <Button variant="primary" onClick={() => close(value)}>
            {opts.confirmLabel ?? 'OK'}
          </Button>
        </>
      }
    >
      {opts.label && <div className="wb-small wb-muted">{opts.label}</div>}
      {opts.multiline ? (
        <TextArea
          ref={inputRef}
          rows={6}
          value={value}
          placeholder={opts.placeholder}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && (e.ctrlKey || e.metaKey) && close(value)}
        />
      ) : (
        <Input
          ref={inputRef}
          value={value}
          placeholder={opts.placeholder}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && close(value)}
        />
      )}
    </Modal>
  )
}
