import { lazy, Suspense, useEffect, useState, useSyncExternalStore } from 'react'
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query'
import { api, setUnauthorizedHandler, settleDeviceKey } from '@/api/client'
import { installResync, startEvents } from '@/api/events'
import { loadHealth } from '@/api/health'
import { installProjectsSync } from '@/api/queries'
import { Login } from '@/shell/Login'
import { providers } from '@/shell/registry'
import '@/state/store' // applies the theme preference to <html> before the first paint
import { ErrorBoundary, ErrorBox, Loading } from '@/ui'

// Each layout loads on its own: a phone never downloads dockview, the desktop never the phone tabs.
const DesktopShell = lazy(() => import('@/shell/DesktopShell').then((m) => ({ default: m.DesktopShell })))
const MobileShell = lazy(() => import('@/shell/MobileShell').then((m) => ({ default: m.MobileShell })))

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 30_000, refetchOnWindowFocus: false, retry: 1 },
  },
})

const mobileQuery = '(max-width: 768px), (pointer: coarse) and (max-width: 1024px)'
function useIsMobile() {
  return useSyncExternalStore(
    (cb) => {
      const m = matchMedia(mobileQuery)
      m.addEventListener('change', cb)
      return () => m.removeEventListener('change', cb)
    },
    () => matchMedia(mobileQuery).matches || new URLSearchParams(location.search).has('mobile'),
  )
}

function Authenticated() {
  const mobile = useIsMobile()
  useEffect(() => startEvents(), [])
  // Once for the whole app, not once per hook instance.
  useEffect(() => installResync(queryClient), [])
  useEffect(() => installProjectsSync(queryClient), [])
  // What the server's OS leaves out (dev containers on Windows…): hidden, or explained.
  useEffect(() => void loadHealth(), [])
  const tree = <Suspense fallback={<Loading label="Loading…" />}>{mobile ? <MobileShell /> : <DesktopShell />}</Suspense>
  return providers.reduceRight((children, P) => <P>{children}</P>, tree)
}

function Root() {
  const [authed, setAuthed] = useState<boolean | null>(null)
  const status = useQuery({
    queryKey: ['auth-status'],
    queryFn: async () => {
      await settleDeviceKey()
      return api.get<{ authenticated: boolean }>('/api/auth/status')
    },
    retry: false,
  })
  useEffect(() => {
    if (status.data) setAuthed(status.data.authenticated)
  }, [status.data])
  useEffect(() => setUnauthorizedHandler(() => setAuthed(false)), [])

  if (authed === null) return status.error ? <Login onDone={() => location.reload()} /> : <Loading label="Connecting…" />
  if (!authed) return <Login onDone={() => location.reload()} />
  // Last resort: a render error that nothing closer contained shows why, not a blank page.
  return (
    <ErrorBoundary
      label="app"
      fallback={(error) => (
        <div className="wb-empty" style={{ minHeight: '100vh' }}>
          <div style={{ width: 'min(560px, 100%)', textAlign: 'left' }}>
            <ErrorBox error={error} onRetry={() => location.reload()} />
          </div>
        </div>
      )}
    >
      <Authenticated />
    </ErrorBoundary>
  )
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <Root />
    </QueryClientProvider>
  )
}
