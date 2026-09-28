// "Ask the agent" from anywhere: a failed CI job, a Confluence page, a diff, an
// editor selection. The terminals slice implements POST /api/agents/ask, which
// pastes the prompt into the project's most recent agent session (or starts one)
// and returns that terminal, which we then focus.

import { api } from '@/api/client'
import type { TerminalInfo } from '@/api/types'
import { isMobileShell, openPanel, toast, toastError } from './actions'

export interface AskAgentOptions {
  projectId: string | null
  prompt: string
  /** Target a specific agent terminal. */
  terminalId?: string
  /** Always start a new session. */
  newSession?: boolean
  /** Name for a new session. */
  name?: string
  /** Press Enter after pasting (default true). */
  submit?: boolean
}

export async function askAgent(o: AskAgentOptions): Promise<TerminalInfo | null> {
  try {
    const term = await api.post<TerminalInfo>('/api/agents/ask', {
      projectId: o.projectId,
      prompt: o.prompt,
      terminalId: o.terminalId,
      newSession: o.newSession ?? false,
      name: o.name,
      submit: o.submit ?? true,
    })
    openPanel({ kind: 'terminal', id: `terminal:${term.id}`, title: term.title, params: { terminalId: term.id } })
    if (isMobileShell()) toast('success', `Sent to ${term.title}`)
    return term
  } catch (e) {
    toastError(e, 'Could not reach an agent')
    return null
  }
}
