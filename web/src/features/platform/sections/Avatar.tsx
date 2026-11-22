// Settings › Avatar: which sessions may show an avatar (`[avatar]` of config.toml, saved like
// every other section), and how avatars are shown and heard in this browser. The second
// part is the avatar slice's own preferences, read and changed through its two contract
// modules (`store` and `voices`): they apply at once and are never sent to the server.
// The phone has no Settings panel: its More tab shows `AvatarMobile`, the two switches that
// matter there.

import { useMemo, useState } from 'react'
import { RotateCcw, Save, Volume2 } from 'lucide-react'
import { useAvatarPrefs } from '@/features/avatar/store'
import { testVoice, useBrowserVoices, type TestVoiceResult } from '@/features/avatar/voices'
import { toastError } from '@/shell/actions'
import { Button, Checkbox, ErrorBox, Loading, Select } from '@/ui'
import { patchSettings, reportApply, useSettings } from '../api'
import {
  AVATAR_CHOICES,
  ONLINE_VOICES,
  SPEED,
  VOLUME,
  avatarChoice,
  avatarPatch,
  choiceHint,
  countVoices,
  offersOnline,
  speedText,
  testText,
  voiceOptions,
  voiceStatus,
  voicesText,
  volumeText,
  type AvatarChoice,
} from '../avatarLib'
import { Group, Note, Page, Row, Segmented, useDraft } from '../common'

/** A switch with a state of its own for "not chosen yet": then neither side is lit. */
type Switch = 'on' | 'off' | ''
const ON_OFF: { value: Switch; label: string }[] = [
  { value: 'on', label: 'On' },
  { value: 'off', label: 'Off' },
]
const CHOICES = AVATAR_CHOICES.map((c) => ({ value: c.value, label: c.label }))

const toneClass = (tone: 'muted' | 'warning'): string => (tone === 'warning' ? 'wb-small wb-warning' : 'wb-small wb-muted')

/** A native slider in the accent colour, with its value beside it. */
function Slider({ label, value, range, text, onChange }: { label: string; value: number; range: { min: number; max: number; step: number }; text: string; onChange: (value: number) => void }) {
  return (
    <>
      <input
        type="range"
        aria-label={label}
        aria-valuetext={text}
        min={range.min}
        max={range.max}
        step={range.step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        style={{ accentColor: 'var(--accent)', width: 220, maxWidth: '100%' }}
      />
      <span className="wb-small wb-muted">{text}</span>
    </>
  )
}

/** "On this device": the avatar preferences of this browser. Every change applies at once, in each of its windows. */
function DeviceGroup() {
  const prefs = useAvatarPrefs()
  const voices = useBrowserVoices()
  const counts = useMemo(() => countVoices(voices), [voices])
  const options = useMemo(() => voiceOptions(voices), [voices])
  const [picked, setPicked] = useState('')
  const [tested, setTested] = useState<TestVoiceResult | null>(null)
  // A voice the browser no longer lists is not tested by name.
  const voiceName = options.some((o) => o.value === picked) ? picked : ''
  const status = voiceStatus(prefs, counts)
  const result = tested ? testText(tested) : null
  return (
    <Group title="On this device" description="Kept in this browser. A change applies at once, in every window of it.">
      <Row label="Avatars" hint="When off, nothing is shown and nothing is said in this browser.">
        <Checkbox checked={prefs.show} onChange={(show) => prefs.setPrefs({ show })}>
          Show avatars
        </Checkbox>
      </Row>
      <Row label="Voice" hint="Avatars say their lines with one of this browser’s voices. Captions are always shown.">
        <Segmented<Switch> value={prefs.voice === null ? '' : prefs.voice ? 'on' : 'off'} onChange={(v) => prefs.setPrefs({ voice: v === 'on' })} options={ON_OFF} />
        <span className={toneClass(status.tone)}>{status.text}</span>
      </Row>
      <Row label="Online voices" hint={ONLINE_VOICES}>
        <Checkbox checked={prefs.online} disabled={counts.online === 0 && !prefs.online} onChange={(online) => prefs.setPrefs({ online })}>
          Allow online voices
        </Checkbox>
        <span className="wb-small wb-muted">{voicesText(counts)}</span>
      </Row>
      <Row label="Voices" hint="An avatar gets one that fits its language. Test voice says one fixed sentence at the speed and volume set here.">
        <Select
          aria-label="Voice to test"
          value={voiceName}
          onChange={(e) => {
            setPicked(e.target.value)
            setTested(null)
          }}
        >
          <option value="">The voice an avatar gets</option>
          {options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </Select>
        {/* The sentence is said from this click: a browser speaks only when the user asked for it. */}
        <Button icon={Volume2} onClick={() => setTested(testVoice(voiceName || undefined))}>
          Test voice
        </Button>
        {result && (
          <span className={toneClass(result.tone)} role="status">
            {result.text}
          </span>
        )}
      </Row>
      <Row label="Speed" hint="An avatar’s own pace is multiplied by it.">
        <Slider label="Speed" value={prefs.speed} range={SPEED} text={speedText(prefs.speed)} onChange={(speed) => prefs.setPrefs({ speed })} />
      </Row>
      <Row label="Volume">
        <Slider label="Volume" value={prefs.volume} range={VOLUME} text={volumeText(prefs.volume)} onChange={(volume) => prefs.setPrefs({ volume })} />
      </Row>
      <Row label="Walking" hint="Avatars stroll about the workspace window, visit and greet each other, and stand still while they speak or you point at them. Drag one to put it somewhere. Off: they stand in a column.">
        <Checkbox checked={prefs.roam} onChange={(roam) => prefs.setPrefs({ roam })}>
          Walk around
        </Checkbox>
      </Row>
      <Row label="Motion" hint="Avatars keep still and stay in their column, apart from the mouth while a line is said. The system’s reduced motion setting does the same.">
        <Checkbox checked={prefs.reduceMotion} onChange={(reduceMotion) => prefs.setPrefs({ reduceMotion })}>
          Reduce motion
        </Checkbox>
      </Row>
    </Group>
  )
}

export function AvatarSection() {
  const settings = useSettings()
  const saved = settings.data ? avatarChoice(settings.data.config.avatar) : undefined
  const { draft: choice, setDraft: setChoice, dirty, reset } = useDraft<AvatarChoice>(saved)
  const [busy, setBusy] = useState(false)

  const save = async () => {
    if (!choice) return
    setBusy(true)
    try {
      reportApply(await patchSettings({ avatar: avatarPatch(choice) }, settings.data?.hash), 'Avatar settings saved')
    } catch (e) {
      toastError(e, 'Could not save')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Page
      title="Avatar"
      description="A session can show a live avatar: a small animated character over the window that says its short messages aloud or as captions and takes answers. Which sessions may have one is saved in config.toml; how avatars are shown and heard is kept in this browser."
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
      <Group title="Sessions">
        {settings.error ? (
          <div style={{ padding: 10 }}>
            <ErrorBox error={settings.error} onRetry={() => void settings.refetch()} />
          </div>
        ) : !choice ? (
          <Loading />
        ) : (
          <Row label="Sessions may show an avatar" hint={choiceHint(choice)}>
            <Segmented value={choice} onChange={setChoice} options={CHOICES} />
          </Row>
        )}
      </Group>

      {saved === 'off' && <Note>Sessions may not show an avatar now: what follows applies once they may.</Note>}

      <DeviceGroup />
    </Page>
  )
}

/**
 * One row of the phone's More tab: what it switches and how that stands on the left, the
 * switch on the right. The switch gets a width of its own: its wrapper (common.tsx) takes
 * the whole row otherwise and leaves the words none.
 */
function MoreSwitch({ title, note, warning, value, onChange }: { title: string; note: string; warning?: boolean; value: Switch; onChange: (on: boolean) => void }) {
  return (
    <div className="wb-more-row">
      <span className="wb-grow">
        <div>{title}</div>
        <div className={warning ? 'wb-xs wb-warning' : 'wb-xs wb-subtle'}>{note}</div>
      </span>
      <div style={{ flex: 'none', width: '7.5em', textAlign: 'right' }}>
        <Segmented<Switch> value={value} onChange={(v) => onChange(v === 'on')} options={ON_OFF} />
      </div>
    </div>
  )
}

/**
 * The phone's More tab: Show avatars and Voice, as rows of its Avatar section. Nothing is
 * shown while avatars are switched off for every session. Online voices are offered where
 * voice cannot work without them, with the sentence that says what allowing them means.
 */
export function AvatarMobile() {
  const settings = useSettings()
  const prefs = useAvatarPrefs()
  const voices = useBrowserVoices()
  const counts = useMemo(() => countVoices(voices), [voices])
  if (settings.data && avatarChoice(settings.data.config.avatar) === 'off') return null
  const status = voiceStatus(prefs, counts)
  return (
    <section>
      <h3 className="wb-more-title">Avatar</h3>
      <div className="wb-set-box">
        <MoreSwitch title="Show avatars" note={prefs.show ? 'Shown over the app.' : 'Hidden: nothing is shown or said here.'} value={prefs.show ? 'on' : 'off'} onChange={(show) => prefs.setPrefs({ show })} />
        <MoreSwitch title="Voice" note={status.text} warning={status.tone === 'warning'} value={prefs.voice === null ? '' : prefs.voice ? 'on' : 'off'} onChange={(voice) => prefs.setPrefs({ voice })} />
        {offersOnline(prefs, counts) && <MoreSwitch title="Allow online voices" note={ONLINE_VOICES} value={prefs.online ? 'on' : 'off'} onChange={(online) => prefs.setPrefs({ online })} />}
      </div>
    </section>
  )
}
