// Symbol kind badges, JetBrains style: a letter in a tinted circle (types in squares).

import { symbolKindName } from './convert'

const KIND: Record<number, { letter: string; tone: string; square?: boolean }> = {
  1: { letter: 'F', tone: 'muted' },
  2: { letter: 'M', tone: 'keyword', square: true },
  3: { letter: 'N', tone: 'keyword', square: true },
  4: { letter: 'P', tone: 'keyword', square: true },
  5: { letter: 'C', tone: 'type', square: true },
  6: { letter: 'm', tone: 'function' },
  7: { letter: 'p', tone: 'variable' },
  8: { letter: 'f', tone: 'variable' },
  9: { letter: 'c', tone: 'function' },
  10: { letter: 'E', tone: 'type', square: true },
  11: { letter: 'I', tone: 'interface', square: true },
  12: { letter: 'f', tone: 'function' },
  13: { letter: 'v', tone: 'muted' },
  14: { letter: 'k', tone: 'number' },
  15: { letter: 's', tone: 'string' },
  16: { letter: '#', tone: 'number' },
  17: { letter: 'b', tone: 'number' },
  18: { letter: 'a', tone: 'muted' },
  19: { letter: 'o', tone: 'muted' },
  20: { letter: 'k', tone: 'muted' },
  21: { letter: '∅', tone: 'muted' },
  22: { letter: 'e', tone: 'variable' },
  23: { letter: 'S', tone: 'type', square: true },
  24: { letter: 'ε', tone: 'function' },
  25: { letter: '±', tone: 'muted' },
  26: { letter: 'T', tone: 'type', square: true },
}

export function SymbolIcon({ kind }: { kind: number }) {
  const k = KIND[kind] ?? { letter: '·', tone: 'muted' }
  return (
    <span className={`lsp-sym ${k.tone}${k.square ? ' square' : ''}`} title={symbolKindName(kind)} aria-label={symbolKindName(kind)}>
      {k.letter}
    </span>
  )
}
