import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, CircleArrowUp, Download, ExternalLink, RefreshCw, RotateCw } from 'lucide-react'
import { FEATURES, useUnsupported } from '@/api/health'
import { toastError } from '@/shell/actions'
import { Button, Checkbox, ErrorBox, Loading, Markdown, Spinner, TimeAgo } from '@/ui'
import { patchSettings, reportApply, useSettings, useUpdate } from '../api'
import { Group, Note, Page, Row } from '../common'
import { updateFraction, updatePhaseText } from '../lib'
import type { UpdateStatus } from '../types'
import { checkForUpdates, installUpdate, restartWorkbench } from '../update'

/** What runs right now: the text and, while downloading, how far it is. */
export function UpdateProgress({ status }: { status: UpdateStatus }) {
  const text = updatePhaseText(status.phase, status.progress, status.latest?.version)
  if (!text) return null
  const fraction = updateFraction(status.phase, status.progress)
  return (
    <div className="wb-set-progress" role="status">
      <div className="wb-row" style={{ gap: 8 }}>
        <Spinner />
        <span>{text}</span>
      </div>
      {fraction !== null && (
        <div className="wb-set-progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(fraction * 100)}>
          <div className="wb-set-progress-bar" style={{ width: `${fraction * 100}%` }} />
        </div>
      )}
    </div>
  )
}

function ReleaseLink({ url }: { url: string }) {
  return (
    <a className="wb-row wb-small" style={{ gap: 4 }} href={url} target="_blank" rel="noopener noreferrer">
      Release page <ExternalLink size={12} />
    </a>
  )
}

/** The line under "Latest release": what the last look found. */
function LatestLine({ s }: { s: UpdateStatus }) {
  const looked = s.checkedAt ? (
    <span className="wb-small wb-muted">
      looked <TimeAgo time={s.checkedAt} />
    </span>
  ) : null
  if (s.available && s.latest) {
    return (
      <>
        <span className="wb-row" style={{ gap: 6 }}>
          <CircleArrowUp size={15} className="wb-set-accent" />
          <span>
            <span className="mono">{s.latest.version}</span> is available
          </span>
        </span>
        {s.latest.publishedAt && (
          <span className="wb-small wb-muted">
            released <TimeAgo time={s.latest.publishedAt} />
          </span>
        )}
        {s.latest.url && <ReleaseLink url={s.latest.url} />}
      </>
    )
  }
  if (s.latest) {
    return (
      <>
        <span className="wb-row wb-success" style={{ gap: 6 }}>
          <CheckCircle2 size={15} /> Up to date
        </span>
        {looked}
      </>
    )
  }
  return <span className="wb-muted">{s.checkedAt ? 'Unknown' : 'Not looked yet'}</span>
}

export function UpdatesSection() {
  const qc = useQueryClient()
  const update = useUpdate()
  const settings = useSettings()
  const noSelfUpdate = useUnsupported(FEATURES.selfUpdate)
  const [checking, setChecking] = useState(false)
  const [saving, setSaving] = useState(false)

  if (update.error) return <ErrorBox error={update.error} onRetry={() => void update.refetch()} />
  const s = update.data
  if (!s) return <Loading />

  const busy = s.phase !== 'idle'
  const check = async () => {
    setChecking(true)
    await checkForUpdates(qc)
    setChecking(false)
  }
  const setDaily = async (on: boolean) => {
    setSaving(true)
    try {
      reportApply(await patchSettings({ update: { check: on } }, settings.data?.hash), on ? 'Workbench looks for updates once a day' : 'Workbench looks for updates only when asked')
    } catch (e) {
      toastError(e, 'Could not save')
    } finally {
      setSaving(false)
    }
  }
  // An install that finished without the restart, or an installer run by hand.
  const restartOffered = !busy && s.restartPending && s.canRestart
  const latest = s.latest

  return (
    <Page
      title="Updates"
      description="Workbench looks for a newer release of itself, and installs one when you say so: it downloads the release, checks its SHA-256, replaces its own program and restarts. Nothing is installed by itself, and agents cannot start it."
      actions={
        <>
          <Button icon={RefreshCw} loading={checking || s.phase === 'checking'} disabled={!s.source || busy} onClick={() => void check()}>
            Check now
          </Button>
          {restartOffered ? (
            <Button variant="primary" icon={RotateCw} onClick={() => void restartWorkbench(qc, s.installed)}>
              Restart now
            </Button>
          ) : (
            s.available &&
            latest && (
              <Button
                variant="primary"
                icon={Download}
                disabled={!s.canInstall || busy}
                title={s.canInstall ? undefined : (s.installNote ?? undefined)}
                onClick={() => void installUpdate(qc, latest.version)}
              >
                Update and restart
              </Button>
            )
          )}
        </>
      }
    >
      {busy && s.phase !== 'checking' && <UpdateProgress status={s} />}
      {!busy && s.failure && (
        <Note tone="warning">
          <strong>The update was not installed.</strong> {sentence(s.failure)}
        </Note>
      )}
      {restartOffered && (
        <Note icon={RotateCw}>
          {s.installed ? (
            <>
              Workbench <span className="mono">{s.installed}</span> is installed.
            </>
          ) : (
            <>The Workbench program on disk was replaced.</>
          )}{' '}
          Restart to use it: the running <span className="mono">{s.current}</span> stays until then.
        </Note>
      )}

      <Group title="This Workbench">
        <Row label="Version">
          <span className="mono">{s.current}</span>
          {settings.data && (
            <span className="wb-small wb-muted">
              started <TimeAgo time={settings.data.startedAt} />
            </span>
          )}
        </Row>
        <Row
          label="Latest release"
          hint={s.source ? `From the GitHub repository ${s.source}.` : undefined}
        >
          {s.source ? <LatestLine s={s} /> : <span className="wb-muted">No release source</span>}
        </Row>
        <Row
          label="Look for updates"
          hint="One request a day to GitHub for the latest release's description, without a token. Off: only when you press Check now."
        >
          <Checkbox checked={s.check} disabled={saving || !settings.data} onChange={(on) => void setDaily(on)}>
            Once a day
          </Checkbox>
        </Row>
      </Group>

      {(s.sourceError || !s.source) && (
        <Note tone={s.sourceError ? 'warning' : undefined}>
          {s.sourceError ? (
            sentence(s.sourceError)
          ) : (
            <>
              This build was not made by the release workflow, so it does not know where its releases are published. To get updates, name the GitHub repository in
              config.toml: <code>[update]</code> <code>repo = "owner/name"</code>.
            </>
          )}
        </Note>
      )}
      {s.source && s.error && (
        <Note tone="warning">
          <strong>The last look failed.</strong> {sentence(s.error)}
        </Note>
      )}
      {s.available && !s.canInstall && s.installNote && !busy && (
        <Note>
          {noSelfUpdate ?? sentence(s.installNote)}
          {latest?.url && (
            <>
              {' '}
              <ReleaseLink url={latest.url} />
            </>
          )}
        </Note>
      )}

      {s.available && latest && (
        <Group title={`What is new in ${latest.version}`}>
          <div className="wb-set-notes">{latest.notes.trim() ? <Markdown text={latest.notes} /> : <span className="wb-muted">This release has no notes.</span>}</div>
        </Group>
      )}
    </Page>
  )
}

/** The server writes reasons like its error messages (lowercase first letter, no full stop). */
function sentence(s: string): string {
  const t = s.trim()
  if (!t) return t
  return t[0].toUpperCase() + t.slice(1) + (/[.!?]$/.test(t) ? '' : '.')
}
