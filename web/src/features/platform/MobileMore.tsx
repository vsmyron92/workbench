// Phone "More" tab: theme, push notifications, devices (pair / revoke), recent activity, sign out.

import { lazy, Suspense, useState } from 'react'
import { ArrowLeft, CircleHelp, LogOut, Moon, QrCode, Sun } from 'lucide-react'
import { api } from '@/api/client'
import { useMobileHelp } from '@/features/help/mobile'
import { toastError } from '@/shell/actions'
import { useUi } from '@/state/store'
import { Button, ErrorBox, Loading } from '@/ui'
import { ActivityList } from './Activity'
import { useRemote, useSettings } from './api'
import { Segmented } from './common'
import { openPairDialog } from './PairDialog'
import { clearWorkerKeys } from './push'
import { PushMobile } from './sections/Push'
import { DevicesList } from './sections/Remote'
import './platform.css'

const HelpView = lazy(() => import('@/features/help/HelpPanel').then((m) => ({ default: m.HelpView })))

export function MobileMore({ projectId }: { projectId: string | null }) {
  const theme = useUi((s) => s.prefs.theme)
  const setPrefs = useUi((s) => s.setPrefs)
  const remote = useRemote()
  const settings = useSettings()
  const [signingOut, setSigningOut] = useState(false)
  const help = useMobileHelp()

  const signOut = async () => {
    setSigningOut(true)
    try {
      await api.post('/api/auth/logout')
      // The session ended, and with it this phone's push; the worker's copy of the key goes too.
      await clearWorkerKeys()
      location.replace('/')
    } catch (e) {
      toastError(e, 'Could not sign out')
      setSigningOut(false)
    }
  }

  if (help.open) {
    return (
      <div className="wb-more">
        <Button icon={ArrowLeft} onClick={help.close} style={{ alignSelf: 'flex-start' }}>
          More
        </Button>
        <Suspense fallback={<Loading />}>
          <HelpView slug={help.slug} onPage={help.setSlug} compact />
        </Suspense>
      </div>
    )
  }

  return (
    <div className="wb-more">
      <section>
        <Button icon={CircleHelp} onClick={() => help.show()} style={{ width: '100%' }}>
          Help
        </Button>
      </section>

      <section>
        <h3 className="wb-more-title">Appearance</h3>
        <div className="wb-set-box">
          <div className="wb-more-row">
            <span className="wb-grow">Theme</span>
            <Segmented
              value={theme}
              onChange={(t) => setPrefs({ theme: t })}
              options={[
                { value: 'dark', label: 'Dark', icon: Moon },
                { value: 'light', label: 'Light', icon: Sun },
              ]}
            />
          </div>
        </div>
      </section>

      <section>
        <h3 className="wb-more-title">Notifications</h3>
        <PushMobile />
      </section>

      <section>
        <div className="wb-row" style={{ marginBottom: 6 }}>
          <h3 className="wb-more-title wb-grow" style={{ margin: 0 }}>
            Devices
          </h3>
          <Button size="small" icon={QrCode} onClick={openPairDialog}>
            Pair another
          </Button>
        </div>
        <div className="wb-set-box">
          {remote.error ? <ErrorBox error={remote.error} /> : !remote.data ? <Loading /> : <DevicesList devices={remote.data.devices} compact />}
        </div>
      </section>

      <section>
        <h3 className="wb-more-title">Activity</h3>
        <div className="wb-set-box" style={{ maxHeight: 360, overflow: 'auto' }}>
          <ActivityList projectId={projectId} compact limit={60} />
        </div>
      </section>

      <section>
        <Button icon={LogOut} loading={signingOut} onClick={() => void signOut()} style={{ width: '100%' }}>
          Sign out of this device
        </Button>
        {settings.data && (
          <div className="wb-xs wb-subtle" style={{ textAlign: 'center', marginTop: 8 }}>
            Workbench {settings.data.version} · {remote.data?.requestHost}
          </div>
        )}
      </section>
    </div>
  )
}
