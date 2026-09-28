// Semantic tokens: every server's legend is mapped onto one Workbench legend (the
// token types and modifiers Workbench announces to servers in `initialize`), so one
// Monaco provider and one set of theme rules serve every language.

/** The order Monaco and the themes (lib/monacoSetup.ts) know. Keep in sync with server/src/lsp/server.rs. */
export const TOKEN_TYPES = [
  'namespace', 'type', 'class', 'enum', 'interface', 'struct', 'typeParameter', 'parameter', 'variable', 'property',
  'enumMember', 'event', 'function', 'method', 'macro', 'keyword', 'modifier', 'comment', 'string', 'number', 'regexp',
  'operator', 'decorator',
]
/**
 * The legend Monaco gets: the same order, but `variable` is `localVariable`, because
 * theme rules match by name and the Monarch grammars' `variable` rule (shell `$VARS`)
 * would otherwise colour every local variable.
 */
export const MONACO_TOKEN_TYPES = TOKEN_TYPES.map((t) => (t === 'variable' ? 'localVariable' : t))
/** Server type names that mean one of ours (typescript-language-server says `member` for methods). */
const TYPE_ALIASES: Record<string, string> = { member: 'method' }

export const TOKEN_MODIFIERS = ['declaration', 'definition', 'readonly', 'static', 'deprecated', 'abstract', 'async', 'modification', 'documentation', 'defaultLibrary']

/**
 * Re-encode LSP relative token data (5 numbers per token: Δline, Δstart, length,
 * type, modifiers) from a server's legend into Workbench's. Tokens of types
 * Workbench does not know are dropped; the positions of the rest are recomputed.
 */
export function remapSemanticTokens(data: ArrayLike<number>, serverTypes: string[], serverModifiers: string[]): Uint32Array {
  const typeMap = serverTypes.map((t) => TOKEN_TYPES.indexOf(TYPE_ALIASES[t] ?? t))
  const modMap = serverModifiers.map((m) => TOKEN_MODIFIERS.indexOf(m))
  const out: number[] = []
  let line = 0
  let start = 0
  let outLine = 0
  let outStart = 0
  for (let i = 0; i + 4 < data.length; i += 5) {
    const dLine = data[i]
    line += dLine
    start = dLine === 0 ? start + data[i + 1] : data[i + 1]
    const type = typeMap[data[i + 3]] ?? -1
    if (type < 0) continue
    let mods = 0
    let bits = data[i + 4]
    for (let b = 0; bits && b < modMap.length; b++, bits >>>= 1) {
      if (bits & 1 && modMap[b] >= 0) mods |= 1 << modMap[b]
    }
    out.push(line - outLine, line === outLine ? start - outStart : start, data[i + 2], type, mods)
    outLine = line
    outStart = start
  }
  return Uint32Array.from(out)
}
