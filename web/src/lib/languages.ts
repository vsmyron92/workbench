// File name → Monaco language id. Pure (no Monaco import), so tests and the lazy
// callers of monacoSetup share one table.

const BY_EXT: Record<string, string> = {
  rs: 'rust', ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  json: 'json', jsonc: 'json', json5: 'json', cs: 'csharp',
  // C and C++ share Monaco's grammar; headers are C++ as in VS Code and CLion.
  c: 'c', h: 'cpp', cc: 'cpp', cpp: 'cpp', cxx: 'cpp', 'c++': 'cpp', hpp: 'cpp', hh: 'cpp', hxx: 'cpp', 'h++': 'cpp',
  ipp: 'cpp', tpp: 'cpp', txx: 'cpp', inl: 'cpp', ixx: 'cpp', cppm: 'cpp', ino: 'cpp', cu: 'cpp', cuh: 'cpp',
  v: 'verilog', vh: 'verilog', sv: 'systemverilog', svh: 'systemverilog',
  vhd: 'vhdl', vhdl: 'vhdl', vho: 'vhdl', vht: 'vhdl',
  go: 'go', java: 'java', kt: 'kotlin', kts: 'kotlin', yml: 'yaml', yaml: 'yaml', md: 'markdown', markdown: 'markdown',
  sh: 'shell', bash: 'shell', zsh: 'shell', env: 'shell', sql: 'sql', ini: 'ini', cfg: 'ini', conf: 'ini',
  html: 'html', htm: 'html', xml: 'xml', svg: 'xml', csproj: 'xml', sln: 'ini', css: 'css', scss: 'css', less: 'css',
  py: 'python', toml: 'toml', lock: 'toml', http: 'http', rest: 'http',
}

/** Monaco language id for a file name. */
export function languageFor(path: string): string {
  const name = path.split('/').pop() ?? ''
  const lower = name.toLowerCase()
  if (lower === 'dockerfile' || lower.startsWith('dockerfile.')) return 'dockerfile'
  if (lower === 'makefile') return 'shell'
  if (lower === 'caddyfile') return 'shell'
  const ext = lower.includes('.') ? lower.split('.').pop()! : ''
  return BY_EXT[ext] ?? 'plaintext'
}
