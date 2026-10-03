import { useState } from 'react'
import { RotateCcw, Save } from 'lucide-react'
import { toastError } from '@/shell/actions'
import { Button, Checkbox, ErrorBox, Input, Loading, Select } from '@/ui'
import { patchSettings, reportApply, useSettings } from '../api'
import { Group, Note, Page, Row, useDraft } from '../common'
import type { GlobalConfig } from '../types'
import { AccountsGroup } from './Accounts'

type Agents = GlobalConfig['agents']

const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max']
const MODES: { value: string; label: string }[] = [
  { value: 'acceptEdits', label: 'Accept edits' },
  { value: 'auto', label: 'Auto' },
  { value: 'plan', label: 'Plan' },
  { value: 'manual', label: 'Manual (ask for everything)' },
  { value: 'dontAsk', label: "Don't ask" },
  { value: 'bypassPermissions', label: 'Bypass permissions' },
]

const normalize = (a: Agents): Agents => ({
  command: a.command.trim() || 'claude',
  model: a.model?.trim() || null,
  effort: a.effort || null,
  permission_mode: a.permission_mode || null,
  remote_control: a.remote_control,
  restore_on_start: a.restore_on_start,
  statusline: a.statusline,
})

export function AgentsSection({ onGoto }: { onGoto: (section: string) => void }) {
  const settings = useSettings()
  const { draft: form, setDraft: setForm, dirty, reset } = useDraft(settings.data?.config.agents, normalize)
  const [busy, setBusy] = useState(false)

  if (settings.error) return <ErrorBox error={settings.error} onRetry={() => void settings.refetch()} />
  if (!form) return <Loading />

  const set = (p: Partial<Agents>) => setForm({ ...form, ...p })
  const save = async () => {
    setBusy(true)
    try {
      reportApply(await patchSettings({ agents: normalize(form) }, settings.data?.hash), 'Agent defaults saved')
    } catch (e) {
      toastError(e, 'Could not save')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Page
      title="Agents"
      description="Defaults for new Claude Code sessions started from Workbench. Each can be changed when starting a session."
      actions={
        <>
          <Button icon={RotateCcw} disabled={!dirty || busy} onClick={reset}>
            Revert
          </Button>
          <Button variant="primary" icon={Save} disabled={!dirty} loading={busy} onClick={() => void save()}>
            Save
          </Button>
        </>
      }
    >
      <Group title="Claude Code">
        <Row label="Command" hint="The Claude Code executable (name on PATH or a full path).">
          <Input className="mono" value={form.command} onChange={(e) => set({ command: e.target.value })} placeholder="claude" />
        </Row>
        <Row label="Model" hint="Alias or full model name. Empty: Claude Code's own default.">
          <Input value={form.model ?? ''} onChange={(e) => set({ model: e.target.value })} placeholder="default" list="wb-agent-models" />
          <datalist id="wb-agent-models">
            <option value="fable" />
            <option value="opus" />
            <option value="sonnet" />
            <option value="haiku" />
          </datalist>
        </Row>
        <Row label="Effort">
          <Select value={form.effort ?? ''} onChange={(e) => set({ effort: e.target.value })}>
            <option value="">Default</option>
            {EFFORTS.map((e) => (
              <option key={e} value={e}>
                {e}
              </option>
            ))}
          </Select>
        </Row>
        <Row label="Permission mode">
          <Select value={form.permission_mode ?? ''} onChange={(e) => set({ permission_mode: e.target.value })}>
            <option value="">Default (from Claude Code settings)</option>
            {MODES.map((m) => (
              <option key={m.value} value={m.value}>
                {m.label}
              </option>
            ))}
          </Select>
        </Row>
      </Group>

      <Group title="Sessions">
        <Row label="Remote Control" hint="Start sessions with --remote-control so they can be continued from claude.ai or the Claude app.">
          <Checkbox checked={form.remote_control} onChange={(v) => set({ remote_control: v })}>
            Enable for new sessions
          </Checkbox>
        </Row>
        <Row label="Restore on start" hint="Resume the sessions that were open when Workbench stopped.">
          <Checkbox checked={form.restore_on_start} onChange={(v) => set({ restore_on_start: v })}>
            Resume open sessions
          </Checkbox>
        </Row>
        <Row label="Status line" hint="Workbench's status line (model, context, cost) unless your own Claude settings define one.">
          <Checkbox checked={form.statusline} onChange={(v) => set({ statusline: v })}>
            Install in hosted sessions
          </Checkbox>
        </Row>
      </Group>

      <div style={{ marginTop: 16 }}>
        <AccountsGroup />
      </div>

      <div style={{ marginTop: 16 }}>
        <Note>
          A project can override model, effort and permission mode in the <code>[agent]</code> table of its <code>.workbench.toml</code> or
          machine overlay —{' '}
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault()
              onGoto('projects')
            }}
          >
            edit project configuration
          </a>
          .
        </Note>
      </div>
    </Page>
  )
}
