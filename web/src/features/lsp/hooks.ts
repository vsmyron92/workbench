// Queries of the lsp slice. `LspProvider` keeps them fresh from `lsp.state` and
// `lsp.diagnostics`.

import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { lspApi, lspKeys } from './api'

export function useLspStatus(projectId: string | null) {
  return useQuery({
    queryKey: lspKeys.status(projectId ?? ''),
    queryFn: ({ signal }) => lspApi.status(projectId!, signal),
    enabled: !!projectId,
    staleTime: 60_000,
    retry: 1,
  })
}

export function useLspDiagnostics(projectId: string | null, enabled = true) {
  return useQuery({
    queryKey: lspKeys.diagnostics(projectId ?? ''),
    queryFn: ({ signal }) => lspApi.diagnostics(projectId!, signal),
    enabled: !!projectId && enabled,
    placeholderData: keepPreviousData,
    staleTime: 10_000,
  })
}
