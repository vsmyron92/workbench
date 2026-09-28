// Breakpoint properties (CLion's breakpoint popup): enabled, condition, hit count,
// and "log a message instead of stopping". Opened from the gutter, the editor
// (Ctrl+Shift+F8 on a breakpoint line) and the Breakpoints list.

import { useState } from 'react'
import { create } from 'zustand'
import { Button, Checkbox, Field, Input, Modal } from '@/ui'
import { breakpointAt, currentSession, removeBreakpoint, updateBreakpoint } from './actions'

interface Target {
  projectId: string
  path: string
  line: number
  focus?: 'condition' | 'log'
}

const useDialog = create<{ target: Target | null; set: (t: Target | null) => void }>()((set) => ({ target: null, set: (target) => set({ target }) }))

export function openBreakpointDialog(projectId: string, path: string, line: number, o?: { focus?: 'condition' | 'log' }) {
  useDialog.getState().set({ projectId, path, line, focus: o?.focus })
}

function Dialog({ t, onClose }: { t: Target; onClose: () => void }) {
  const bp = breakpointAt(t.projectId, t.path, t.line)
  const caps = currentSession(t.projectId)?.capabilities
  const [enabled, setEnabled] = useState(bp?.enabled ?? true)
  const [condition, setCondition] = useState(bp?.condition ?? '')
  const [hit, setHit] = useState(bp?.hitCondition ?? '')
  const [log, setLog] = useState(bp?.logMessage ?? '')
  const [logOn, setLogOn] = useState(t.focus === 'log' || !!bp?.logMessage)
  const save = () => {
    void updateBreakpoint(t.projectId, t.path, t.line, {
      enabled,
      condition: condition.trim() || undefined,
      hitCondition: hit.trim() || undefined,
      logMessage: logOn && log.trim() ? log.trim() : undefined,
    })
    onClose()
  }
  const unsupported = (k: 'supportsConditionalBreakpoints' | 'supportsHitConditionalBreakpoints' | 'supportsLogPoints') => caps && caps[k] === false
  const name = t.path.split('/').pop()
  return (
    <Modal
      title={`${bp ? 'Breakpoint' : 'New breakpoint'} at ${name}:${t.line}`}
      onClose={onClose}
      footer={
        <>
          {bp && (
            <Button
              variant="danger"
              onClick={() => {
                void removeBreakpoint(t.projectId, t.path, t.line)
                onClose()
              }}
            >
              Remove
            </Button>
          )}
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={save}>
            {bp ? 'Save' : 'Add'}
          </Button>
        </>
      }
    >
      <form
        className="wb-dbg-bpform"
        onSubmit={(e) => {
          e.preventDefault()
          save()
        }}
      >
        <Checkbox checked={enabled} onChange={setEnabled}>
          Enabled
        </Checkbox>
        <Field label="Condition" hint={unsupported('supportsConditionalBreakpoints') ? 'This debugger ignores conditions.' : 'Stop only when this expression is true, e.g. i == 3.'}>
          <Input value={condition} onChange={(e) => setCondition(e.target.value)} autoFocus={t.focus !== 'log'} placeholder="expression" spellCheck={false} />
        </Field>
        <Field label="Hit count" hint={unsupported('supportsHitConditionalBreakpoints') ? 'This debugger ignores hit counts.' : 'Stop on this hit, e.g. 5 (gdb), or >= 5 / % 2 where the debugger understands it.'}>
          <Input value={hit} onChange={(e) => setHit(e.target.value)} placeholder="count" spellCheck={false} />
        </Field>
        <Checkbox checked={logOn} onChange={setLogOn}>
          Log a message instead of stopping
        </Checkbox>
        {logOn && (
          <Field label="Message" hint={unsupported('supportsLogPoints') ? 'This debugger cannot log: the breakpoint is not placed.' : 'Expressions in {braces} are evaluated, e.g. total is {total}.'}>
            <Input value={log} onChange={(e) => setLog(e.target.value)} autoFocus={t.focus === 'log'} placeholder="message" spellCheck={false} />
          </Field>
        )}
        <button type="submit" hidden />
      </form>
    </Modal>
  )
}

export function BreakpointDialogHost() {
  const { target, set } = useDialog()
  if (!target) return null
  return <Dialog key={`${target.projectId}:${target.path}:${target.line}`} t={target} onClose={() => set(null)} />
}
