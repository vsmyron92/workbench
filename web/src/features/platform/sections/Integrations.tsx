import { useMemo, useState } from 'react'
import { CheckCircle2, FlaskConical, RotateCcw, Save } from 'lucide-react'
import { api, ApiError } from '@/api/client'
import { useProjects } from '@/api/queries'
import { toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Button, Checkbox, ErrorBox, Input, Loading, Select, Spinner } from '@/ui'
import { ConfluenceIcon, GitHubIcon, GitLabIcon } from '@/ui/brand'
import { patchSettings, reportApply, useSettings } from '../api'
import { Group, Page, Row, useDraft } from '../common'
import type { GlobalConfig } from '../types'

interface Form {
  gitlabOn: boolean
  gitlabHost: string
  gitlabToken: string
  githubOn: boolean
  githubHost: string
  /** '' = no token (public repositories, read-only). */
  githubToken: string
  atlOn: boolean
  site: string
  email: string
  atlToken: string
}

function fromConfig(c: GlobalConfig): Form {
  return {
    gitlabOn: !!c.gitlab,
    gitlabHost: c.gitlab?.host ?? 'gitlab.com',
    gitlabToken: c.gitlab?.token ?? 'gitlab',
    githubOn: !!c.github,
    githubHost: c.github?.host ?? 'github.com',
    githubToken: c.github ? c.github.token : 'github' in c.secrets ? 'github' : '',
    atlOn: !!c.atlassian,
    site: c.atlassian?.site ?? '',
    email: c.atlassian?.email ?? '',
    atlToken: c.atlassian?.token ?? 'atlassian',
  }
}

const normalize = (f: Form) =>
  JSON.stringify({
    gitlab: f.gitlabOn ? { host: f.gitlabHost.trim(), token: f.gitlabToken } : null,
    github: f.githubOn ? { host: f.githubHost.trim(), token: f.githubToken } : null,
    atlassian: f.atlOn ? { site: f.site.trim(), email: f.email.trim(), token: f.atlToken } : null,
  })

/** Select a `[secrets]` entry by name. */
function SecretSelect({ value, names, onChange }: { value: string; names: string[]; onChange: (v: string) => void }) {
  const all = names.includes(value) || !value ? names : [value, ...names]
  return (
    <Select value={value} onChange={(e) => onChange(e.target.value)}>
      {!value && <option value="">Choose a secret…</option>}
      {all.map((n) => (
        <option key={n} value={n}>
          {n}
          {names.includes(n) ? '' : ' (not defined)'}
        </option>
      ))}
    </Select>
  )
}

type TestState = { state: 'idle' } | { state: 'running' } | { state: 'ok'; detail: string } | { state: 'error'; error: unknown }

function TestResult({ t }: { t: TestState }) {
  if (t.state === 'running') return <Spinner />
  if (t.state === 'ok')
    return (
      <span className="wb-row wb-small wb-success">
        <CheckCircle2 size={13} /> {t.detail}
      </span>
    )
  return null
}

function describe(v: unknown): string {
  if (!v || typeof v !== 'object') return 'Connection works'
  const o = v as Record<string, unknown>
  const who = [o.user, o.username, o.name, o.displayName, o.email].find((x) => typeof x === 'string')
  return who ? `Connected as ${who as string}` : 'Connection works'
}

/** `/api/github/status` answers 200 without a (working) token; report those as failures. */
function describeGithub(v: unknown): string {
  const s = (v ?? {}) as { configured?: boolean; valid?: boolean; status?: number; error?: string; host?: string; user?: { login?: string } | null }
  if (!s.configured)
    throw new ApiError(412, 'not_configured', s.error ?? 'No token is set: GitHub is used for public repositories only, read-only (60 requests an hour).')
  if (!s.valid) throw new Error(`${s.host ?? 'GitHub'} refused the token${s.status ? ` (HTTP ${s.status})` : ''}.`)
  return s.user?.login ? `Connected as ${s.user.login}` : 'Connection works'
}

/** `/api/atlassian/status` answers 200 even when it found a problem; report those as failures. */
function describeAtlassian(v: unknown): string {
  const s = (v ?? {}) as { configured?: boolean; authFailed?: boolean; confluence?: boolean; jira?: boolean; error?: string | null; user?: { displayName?: string } | null }
  if (s.configured === false || s.authFailed) throw new ApiError(412, 'not_configured', s.error ?? 'Atlassian is not set up')
  if (!s.confluence && !s.jira) throw new Error(s.error ?? 'The site has neither Confluence nor Jira')
  return s.user?.displayName ? `Connected as ${s.user.displayName}` : 'Connection works'
}

export function IntegrationsSection({ onGoto }: { onGoto: (section: string) => void }) {
  const settings = useSettings()
  const { data: projects } = useProjects()
  const currentId = useUi((s) => s.projectId)
  const config = settings.data?.config
  // Memoized: the draft hook follows identity changes of `saved`.
  const saved = useMemo(() => (config ? fromConfig(config) : undefined), [config])
  const { draft: form, setDraft: setForm, dirty, reset } = useDraft(saved, normalize)
  const [busy, setBusy] = useState(false)
  const [glTest, setGlTest] = useState<TestState>({ state: 'idle' })
  const [ghTest, setGhTest] = useState<TestState>({ state: 'idle' })
  const [atlTest, setAtlTest] = useState<TestState>({ state: 'idle' })

  if (settings.error) return <ErrorBox error={settings.error} onRetry={() => void settings.refetch()} />
  if (!form || !settings.data) return <Loading />

  const secretNames = Object.keys(settings.data.config.secrets).sort()
  const set = (p: Partial<Form>) => setForm({ ...form, ...p })
  const gitlabProject = [...(projects ?? [])].sort((a, b) => Number(b.id === currentId) - Number(a.id === currentId)).find((p) => p.gitlab)

  const save = async () => {
    setBusy(true)
    try {
      const r = await patchSettings(
        {
          gitlab: form.gitlabOn ? { host: form.gitlabHost.trim() || 'gitlab.com', token: form.gitlabToken } : null,
          github: form.githubOn ? { host: form.githubHost.trim().replace(/\/+$/, '') || 'github.com', token: form.githubToken } : null,
          atlassian: form.atlOn ? { site: form.site.trim().replace(/\/+$/, ''), email: form.email.trim(), token: form.atlToken } : null,
        },
        settings.data?.hash,
      )
      reportApply(r, 'Integrations saved')
    } catch (e) {
      toastError(e, 'Could not save')
    } finally {
      setBusy(false)
    }
  }

  const run = async (url: string, setT: (t: TestState) => void, check: (v: unknown) => string = describe) => {
    setT({ state: 'running' })
    try {
      setT({ state: 'ok', detail: check(await api.get<unknown>(url)) })
    } catch (e) {
      setT({ state: 'error', error: e })
    }
  }

  return (
    <Page
      title="Integrations"
      description={
        <>
          GitLab, GitHub and Atlassian accounts used by every project unless its config says otherwise. Tokens are{' '}
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault()
              onGoto('secrets')
            }}
          >
            secret references
          </a>{' '}
          — the values stay on this machine.
        </>
      }
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
      <Group
        title={
          <span className="wb-row">
            <GitLabIcon size={15} /> GitLab
          </span>
        }
        actions={
          <>
            <TestResult t={glTest} />
            <Button
              size="small"
              icon={FlaskConical}
              disabled={!gitlabProject || dirty || glTest.state === 'running'}
              title={dirty ? 'Save first' : gitlabProject ? `Reads ${gitlabProject.name}'s GitLab summary` : 'No project with a GitLab remote'}
              onClick={() => gitlabProject && void run(`/api/projects/${encodeURIComponent(gitlabProject.id)}/gitlab/summary`, setGlTest)}
            >
              Test
            </Button>
          </>
        }
      >
        <Row label="Enabled">
          <Checkbox checked={form.gitlabOn} onChange={(v) => set({ gitlabOn: v })}>
            Use GitLab for merge requests, pipelines and issues
          </Checkbox>
        </Row>
        {form.gitlabOn && (
          <>
            <Row label="Host" hint="gitlab.com or your self-managed host name.">
              <Input value={form.gitlabHost} onChange={(e) => set({ gitlabHost: e.target.value })} placeholder="gitlab.com" />
            </Row>
            <Row label="Access token" hint="Personal access token with read_api (api to comment or retry jobs).">
              <SecretSelect value={form.gitlabToken} names={secretNames} onChange={(v) => set({ gitlabToken: v })} />
            </Row>
          </>
        )}
      </Group>
      {glTest.state === 'error' && <ErrorBox error={glTest.error} />}

      <Group
        title={
          <span className="wb-row">
            <GitHubIcon size={15} /> GitHub
          </span>
        }
        actions={
          <>
            <TestResult t={ghTest} />
            <Button
              size="small"
              icon={FlaskConical}
              disabled={!form.githubOn || dirty || ghTest.state === 'running'}
              title={dirty ? 'Save first' : 'Checks the token with GitHub'}
              onClick={() => void run('/api/github/status', setGhTest, describeGithub)}
            >
              Test
            </Button>
          </>
        }
      >
        <Row label="Enabled">
          <Checkbox checked={form.githubOn} onChange={(v) => set({ githubOn: v })}>
            Use GitHub for pull requests, Actions, issues and releases
          </Checkbox>
        </Row>
        {form.githubOn && (
          <>
            <Row label="Host" hint="github.com, or your GitHub Enterprise host name. Projects whose remote is on this host use it.">
              <Input value={form.githubHost} onChange={(e) => set({ githubHost: e.target.value })} placeholder="github.com" />
            </Row>
            <Row
              label="Access token"
              hint="A fine-grained or classic token (repo and workflow scopes to comment, merge or re-run). Without one, public repositories are read-only and limited to 60 requests an hour."
            >
              <Select value={form.githubToken} onChange={(e) => set({ githubToken: e.target.value })}>
                <option value="">No token (public, read-only)</option>
                {(secretNames.includes(form.githubToken) || !form.githubToken ? secretNames : [form.githubToken, ...secretNames]).map((n) => (
                  <option key={n} value={n}>
                    {n}
                    {secretNames.includes(n) ? '' : ' (not defined)'}
                  </option>
                ))}
              </Select>
            </Row>
          </>
        )}
      </Group>
      {ghTest.state === 'error' && <ErrorBox error={ghTest.error} settingsSection="secrets" />}

      <Group
        title={
          <span className="wb-row">
            <ConfluenceIcon size={15} /> Atlassian (Confluence and Jira)
          </span>
        }
        actions={
          <>
            <TestResult t={atlTest} />
            <Button
              size="small"
              icon={FlaskConical}
              disabled={!form.atlOn || dirty || atlTest.state === 'running'}
              title={dirty ? 'Save first' : undefined}
              onClick={() => void run('/api/atlassian/status?refresh=1', setAtlTest, describeAtlassian)}
            >
              Test
            </Button>
          </>
        }
      >
        <Row label="Enabled">
          <Checkbox checked={form.atlOn} onChange={(v) => set({ atlOn: v })}>
            Use an Atlassian Cloud site
          </Checkbox>
        </Row>
        {form.atlOn && (
          <>
            <Row label="Site" hint="https://<site>.atlassian.net">
              <Input value={form.site} onChange={(e) => set({ site: e.target.value })} placeholder="https://example.atlassian.net" />
            </Row>
            <Row label="Email" hint="The Atlassian account the API token belongs to.">
              <Input type="email" value={form.email} onChange={(e) => set({ email: e.target.value })} placeholder="you@example.com" />
            </Row>
            <Row label="API token" hint="From id.atlassian.com → Security → API tokens.">
              <SecretSelect value={form.atlToken} names={secretNames} onChange={(v) => set({ atlToken: v })} />
            </Row>
          </>
        )}
      </Group>
      {atlTest.state === 'error' && <ErrorBox error={atlTest.error} />}
    </Page>
  )
}
