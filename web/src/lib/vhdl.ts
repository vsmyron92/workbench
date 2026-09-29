// VHDL for Monaco, which ships Verilog and SystemVerilog but no VHDL. VHDL-2008/2019:
// case-insensitive words, `--` and `/* */` comments, bit-string (`x"FF"`, `8ux"0F"`)
// and based (`16#FF#`) literals, and a `'` that is either a character literal (`'0'`)
// or an attribute / qualified expression after a name (`clk'event`, `unsigned'(…)`).

import type { languages } from 'monaco-editor'

export const vhdlConfiguration: languages.LanguageConfiguration = {
  comments: { lineComment: '--', blockComment: ['/*', '*/'] },
  brackets: [
    ['(', ')'],
    ['[', ']'],
  ],
  autoClosingPairs: [
    { open: '(', close: ')' },
    { open: '[', close: ']' },
    { open: '"', close: '"', notIn: ['string', 'comment'] },
    { open: "'", close: "'", notIn: ['string', 'comment'] },
  ],
  surroundingPairs: [
    { open: '(', close: ')' },
    { open: '[', close: ']' },
    { open: '"', close: '"' },
  ],
  indentationRules: {
    increaseIndentPattern: /^(?!\s*--).*\b(is|begin|then|else|loop|generate|record|units|protected)\s*(--.*)?$/i,
    decreaseIndentPattern: /^\s*(end|else|elsif|begin)\b/i,
  },
}

export const vhdlLanguage: languages.IMonarchLanguage = {
  defaultToken: '',
  tokenPostfix: '.vhdl',
  ignoreCase: true,
  brackets: [
    { open: '(', close: ')', token: 'delimiter.parenthesis' },
    { open: '[', close: ']', token: 'delimiter.square' },
  ],
  // Reserved words of VHDL-2019 (including PSL's), plus the boolean literals.
  keywords: [
    'abs', 'access', 'after', 'alias', 'all', 'and', 'architecture', 'array', 'assert', 'assume', 'attribute', 'begin',
    'block', 'body', 'buffer', 'bus', 'case', 'component', 'configuration', 'constant', 'context', 'cover', 'default',
    'disconnect', 'downto', 'else', 'elsif', 'end', 'entity', 'exit', 'fairness', 'file', 'for', 'force', 'function',
    'generate', 'generic', 'group', 'guarded', 'if', 'impure', 'in', 'inertial', 'inout', 'is', 'label', 'library',
    'linkage', 'literal', 'loop', 'map', 'mod', 'nand', 'new', 'next', 'nor', 'not', 'null', 'of', 'on', 'open', 'or',
    'others', 'out', 'package', 'parameter', 'port', 'postponed', 'private', 'procedure', 'process', 'property',
    'protected', 'pure', 'range', 'record', 'register', 'reject', 'release', 'rem', 'report', 'restrict', 'return', 'rol',
    'ror', 'select', 'sequence', 'severity', 'shared', 'signal', 'sla', 'sll', 'sra', 'srl', 'strong', 'subtype', 'then',
    'to', 'transport', 'type', 'unaffected', 'units', 'until', 'use', 'variable', 'view', 'vmode', 'vpkg', 'vprop',
    'vunit', 'wait', 'when', 'while', 'with', 'xnor', 'xor', 'true', 'false',
  ],
  // Types of STD and IEEE that designs name everywhere (not TEXTIO's `line`, `text`,
  // `side` and `width`, which are common signal and generic names too).
  types: [
    'bit', 'bit_vector', 'boolean', 'boolean_vector', 'character', 'integer', 'integer_vector', 'natural', 'positive',
    'real', 'real_vector', 'string', 'time', 'time_vector', 'delay_length', 'severity_level', 'file_open_kind',
    'file_open_status', 'std_logic', 'std_logic_vector', 'std_ulogic', 'std_ulogic_vector', 'signed', 'unsigned',
    'sfixed', 'ufixed', 'float', 'float32', 'float64', 'float128',
  ],
  operators: [
    '<=', ':=', '=>', '/=', '>=', '<', '>', '=', '**', '*', '/', '+', '-', '&', '|', '??', '?=', '?/=', '?<', '?<=', '?>',
    '?>=', '<>', ':', '<<', '>>', '@', '^',
  ],
  symbols: /[=><!?:&|+\-*/^@]+/,
  identifier: /[a-z][\w]*/,

  tokenizer: {
    root: [
      { include: '@whitespace' },
      // VHDL-2019 tool directives.
      [/`\w+/, 'keyword.directive'],
      // Bit strings, with the VHDL-2008 width and sign: x"FF", 8ux"0F", b"1010_0101".
      [/\d*[us]?b"[^"\n]*"/, 'number.binary'],
      [/\d*[us]?o"[^"\n]*"/, 'number.octal'],
      [/\d*[us]?x"[^"\n]*"/, 'number.hex'],
      [/\d*d"[^"\n]*"/, 'number'],
      // A name followed by an attribute (clk'event) or a qualified expression (unsigned'(…)).
      [/(@identifier)(')(@identifier)/, [{ cases: { '@keywords': 'keyword', '@types': 'type', '@default': 'identifier' } }, 'delimiter', 'variable.attribute']],
      [/(@identifier)(')(?=\()/, [{ cases: { '@keywords': 'keyword', '@types': 'type', '@default': 'identifier' } }, 'delimiter']],
      [/@identifier/, { cases: { '@keywords': 'keyword', '@types': 'type', '@default': 'identifier' } }],
      // Extended identifiers: \any chars\.
      [/\\([^\\\n]|\\\\)*\\/, 'identifier'],
      // Based literals (16#FF#, 2#1010.1#e3), then decimal ones.
      [/\d[\d_]*#[\da-f_]+(\.[\da-f_]+)?#(e[+-]?\d[\d_]*)?/, 'number'],
      [/\d[\d_]*\.\d[\d_]*(e[+-]?\d[\d_]*)?/, 'number.float'],
      [/\d[\d_]*(e[+-]?\d[\d_]*)?/, 'number'],
      [/'.'/, 'string'],
      [/"([^"\n]|"")*$/, 'string.invalid'],
      [/"/, 'string', '@string'],
      [/[()[\]]/, '@brackets'],
      [/@symbols/, { cases: { '@operators': 'delimiter', '@default': '' } }],
      [/[;,.]/, 'delimiter'],
    ],
    whitespace: [
      [/[ \t\r\n]+/, ''],
      [/--.*$/, 'comment'],
      [/\/\*/, 'comment', '@comment'],
    ],
    comment: [
      [/[^/*]+/, 'comment'],
      [/\*\//, 'comment', '@pop'],
      [/[/*]/, 'comment'],
    ],
    string: [
      [/[^"]+/, 'string'],
      [/""/, 'string.escape'],
      [/"/, 'string', '@pop'],
    ],
  },
}
