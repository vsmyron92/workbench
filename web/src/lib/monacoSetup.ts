// Monaco, bundled locally (no CDN — must work offline and over remote access).
// Imported lazily by ui/monaco.tsx before the first editor mounts.
// monaco-editor >= 0.56 moved its ESM entry points; these paths are verified.

import * as monaco from 'monaco-editor/editor'
import 'monaco-editor/features/register.all'
import 'monaco-editor/languages/definitions/rust/register'
import 'monaco-editor/languages/definitions/typescript/register'
import 'monaco-editor/languages/definitions/javascript/register'
import 'monaco-editor/languages/definitions/csharp/register'
import 'monaco-editor/languages/definitions/cpp/register'
import 'monaco-editor/languages/definitions/go/register'
import 'monaco-editor/languages/definitions/java/register'
import 'monaco-editor/languages/definitions/kotlin/register'
import 'monaco-editor/languages/definitions/yaml/register'
import 'monaco-editor/languages/definitions/markdown/register'
import 'monaco-editor/languages/definitions/shell/register'
import 'monaco-editor/languages/definitions/dockerfile/register'
import 'monaco-editor/languages/definitions/sql/register'
import 'monaco-editor/languages/definitions/ini/register'
import 'monaco-editor/languages/definitions/html/register'
import 'monaco-editor/languages/definitions/xml/register'
import 'monaco-editor/languages/definitions/css/register'
import 'monaco-editor/languages/definitions/python/register'
import 'monaco-editor/languages/features/json/register'
// Whole-document semantic tokens (language servers' `textDocument/semanticTokens/full`):
// `register.all` brings only the viewport (range) variant.
import 'monaco-editor/editor/contrib/semanticTokens/browser/documentSemanticTokens'
import { ContextView } from 'monaco-editor/base/browser/ui/contextview/contextview'
import { MenuId, MenuRegistry } from 'monaco-editor/platform/actions/common/actions'
import { loader } from '@monaco-editor/react'
import { conf as verilogConfiguration, language as verilogLanguage } from 'monaco-editor/languages/definitions/systemverilog/systemverilog'
import { installEditorKeymap } from './editorKeymap'
import { vhdlConfiguration, vhdlLanguage } from './vhdl'
import EditorWorker from 'monaco-editor/editor/editor.worker?worker'
import JsonWorker from 'monaco-editor/languages/features/json/json.worker?worker'

self.MonacoEnvironment = {
  getWorker: (_id: string, label: string) => (label === 'json' ? new JsonWorker() : new EditorWorker()),
}
loader.config({ monaco })

// JetBrains HTTP Client files (features/apps/http): requests, headers, {{variables}}.
monaco.languages.register({ id: 'http', extensions: ['.http', '.rest'], aliases: ['HTTP Request'] })
monaco.languages.setMonarchTokensProvider('http', {
  methods: ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS', 'TRACE', 'CONNECT'],
  tokenizer: {
    root: [
      [/^###.*$/, 'keyword'],
      [/^\s*(#|\/\/).*$/, 'comment'],
      [/^@[\w.-]+/, 'variable'],
      [/\{\{[^}]*\}\}/, 'type'],
      [/^[A-Z]+(?=\s)/, { cases: { '@methods': 'keyword', '@default': '' } }],
      [/https?:\/\/[^\s{]+/, 'string'],
      [/^[\w-]+(?=:)/, 'attribute.name'],
      [/HTTP\/[\d.]+/, 'comment'],
      [/^> \{%/, { token: 'comment', next: '@script' }],
      [/^<\s.*$/, 'string'],
    ],
    script: [
      [/%\}/, { token: 'comment', next: '@pop' }],
      [/./, 'comment'],
    ],
  },
})

// TOML has no built-in grammar (Cargo.toml, .workbench.toml, config.toml).
monaco.languages.register({ id: 'toml', extensions: ['.toml'], aliases: ['TOML'] })
monaco.languages.setMonarchTokensProvider('toml', {
  tokenizer: {
    root: [
      [/^\s*\[\[?[^\]]*\]\]?/, 'type'],
      [/#.*$/, 'comment'],
      [/"""/, 'string', '@mlstring'],
      [/"([^"\\]|\\.)*"/, 'string'],
      [/'[^']*'/, 'string'],
      [/\b(true|false)\b/, 'keyword'],
      [/\d{4}-\d{2}-\d{2}([T ][\d:.]+)?(Z|[+-]\d{2}:\d{2})?/, 'number'],
      [/[+-]?\d[\d_]*(\.\d+)?([eE][+-]?\d+)?/, 'number'],
      [/[A-Za-z0-9_.-]+(?=\s*=)/, 'variable'],
    ],
    mlstring: [
      [/"""/, 'string', '@pop'],
      [/./, 'string'],
    ],
  },
})

// Verilog and SystemVerilog: Monaco's grammar, registered here so that only (), [] and {}
// are brackets. Monaco's configuration also pairs begin/end, module/endmodule,
// property/endproperty…: bracket pair colourisation then paints those over the keyword
// colour, and in red where a keyword has no partner (`assert property`, `extern function`,
// DPI imports). Its folding markers still fold them.
monaco.languages.register({ id: 'verilog', extensions: ['.v', '.vh'], aliases: ['Verilog', 'verilog'] })
monaco.languages.register({ id: 'systemverilog', extensions: ['.sv', '.svh'], aliases: ['SystemVerilog', 'systemverilog'] })
for (const id of ['verilog', 'systemverilog']) {
  monaco.languages.setLanguageConfiguration(id, { ...verilogConfiguration, brackets: [['{', '}'], ['[', ']'], ['(', ')']] })
  monaco.languages.setMonarchTokensProvider(id, verilogLanguage)
}

// VHDL (lib/vhdl.ts).
monaco.languages.register({ id: 'vhdl', extensions: ['.vhd', '.vhdl', '.vho', '.vht'], aliases: ['VHDL', 'vhdl'] })
monaco.languages.setLanguageConfiguration('vhdl', vhdlConfiguration)
monaco.languages.setMonarchTokensProvider('vhdl', vhdlLanguage)

/** The design tokens of one theme, read from tokens.css (whichever theme is showing). */
function themeTokens(theme: 'dark' | 'light') {
  const probe = document.createElement('div')
  probe.dataset.theme = theme
  probe.style.display = 'none'
  document.body.appendChild(probe)
  const style = getComputedStyle(probe)
  const get = (name: string, fallback: string) => toHex(style.getPropertyValue(name).trim()) ?? fallback
  const dark = theme === 'dark'
  const t = {
    bg: get('--bg', dark ? '#1e1f22' : '#ffffff'),
    elevated: get('--bg-elevated', dark ? '#313338' : '#ffffff'),
    hover: get('--bg-hover', dark ? '#393b40' : '#ebecf0'),
    active: get('--bg-active', dark ? '#2e436e' : '#d4e2ff'),
    border: get('--border', dark ? '#393b40' : '#ebecf0'),
    borderStrong: get('--border-strong', dark ? '#4e5157' : '#dfe1e5'),
    fg: get('--fg', dark ? '#dfe1e5' : '#1e1f22'),
    fgMuted: get('--fg-muted', dark ? '#9da0a8' : '#5a5d63'),
    accent: get('--accent', dark ? '#548af7' : '#3574f0'),
    accentStrong: get('--accent-strong', '#3574f0'),
    onAccent: get('--fg-on-accent', '#ffffff'),
    synType: get('--syn-type', dark ? '#16baac' : '#008080'),
    synFunction: get('--syn-function', dark ? '#56a8f5' : '#00627a'),
    synVariable: get('--syn-variable', dark ? '#c77dbb' : '#871094'),
    synMeta: get('--syn-meta', dark ? '#b3ae60' : '#9e880d'),
  }
  probe.remove()
  return t
}

/** `#rgb`, `#rrggbb[aa]` or `rgb[a](…)` → `#rrggbb[aa]` (Monaco only takes hex colours). */
function toHex(value: string): string | null {
  if (/^#[0-9a-f]{3}$/i.test(value)) return '#' + value.slice(1).split('').map((c) => c + c).join('')
  if (/^#[0-9a-f]{6}([0-9a-f]{2})?$/i.test(value)) return value
  const m = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+%?))?\s*\)$/i.exec(value)
  if (!m) return null
  const byte = (n: number) => Math.round(Math.min(255, Math.max(0, n))).toString(16).padStart(2, '0')
  let alpha = ''
  if (m[4] !== undefined) {
    const a = m[4].endsWith('%') ? parseFloat(m[4]) / 100 : parseFloat(m[4])
    if (a < 1) alpha = byte(a * 255)
  }
  return '#' + byte(+m[1]) + byte(+m[2]) + byte(+m[3]) + alpha
}

/**
 * Colours for the language servers' semantic tokens (features/lsp/semanticTokens.ts),
 * after CLion: types teal, functions blue, fields and enum members purple, macros and
 * decorators like annotations; parameters and locals keep the text colour.
 */
function semanticRules(t: ReturnType<typeof themeTokens>): { token: string; foreground?: string; fontStyle?: string }[] {
  const c = (hex: string) => hex.replace('#', '').slice(0, 6)
  return [
    ...['namespace', 'type', 'class', 'enum', 'interface', 'struct', 'typeParameter'].map((token) => ({ token, foreground: c(t.synType) })),
    { token: 'function', foreground: c(t.synFunction) },
    { token: 'method', foreground: c(t.synFunction) },
    { token: 'property', foreground: c(t.synVariable) },
    { token: 'enumMember', foreground: c(t.synVariable), fontStyle: 'italic' },
    { token: 'property.static', foreground: c(t.synVariable), fontStyle: 'italic' },
    { token: 'localVariable.static', fontStyle: 'italic' },
    { token: 'macro', foreground: c(t.synMeta) },
    { token: 'decorator', foreground: c(t.synMeta) },
    { token: 'localVariable.deprecated', fontStyle: 'strikethrough' },
    { token: 'function.deprecated', fontStyle: 'strikethrough' },
    { token: 'method.deprecated', fontStyle: 'strikethrough' },
  ]
}

/** Monaco's menus, hovers, suggestions and find/peek widgets in Workbench's colours. */
function widgetColors(t: ReturnType<typeof themeTokens>): Record<string, string> {
  return {
    'widget.shadow': '#00000055',
    'editorWidget.background': t.elevated,
    'editorWidget.foreground': t.fg,
    'editorWidget.border': t.borderStrong,
    'editorHoverWidget.background': t.elevated,
    'editorHoverWidget.border': t.borderStrong,
    'editorSuggestWidget.background': t.elevated,
    'editorSuggestWidget.border': t.borderStrong,
    'editorSuggestWidget.foreground': t.fg,
    'editorSuggestWidget.selectedBackground': t.active,
    'editorSuggestWidget.highlightForeground': t.accent,
    'editorSuggestWidget.focusHighlightForeground': t.accent,
    'menu.background': t.elevated,
    'menu.foreground': t.fg,
    'menu.border': t.borderStrong,
    'menu.selectionBackground': t.accentStrong,
    'menu.selectionForeground': t.onAccent,
    'menu.separatorBackground': t.border,
    'list.hoverBackground': t.hover,
    'list.activeSelectionBackground': t.active,
    'list.activeSelectionForeground': t.fg,
    'list.inactiveSelectionBackground': t.hover,
    'list.highlightForeground': t.accent,
    'quickInput.background': t.elevated,
    'quickInput.foreground': t.fg,
    'input.background': t.bg,
    'input.border': t.borderStrong,
    'focusBorder': t.accentStrong,
    'peekViewEditor.background': t.bg,
    'peekViewResult.background': t.elevated,
    'peekViewTitle.background': t.elevated,
    'peekView.border': t.accentStrong,
    'descriptionForeground': t.fgMuted,
  }
}

function defineThemes() {
  const dark = themeTokens('dark')
  const light = themeTokens('light')
  monaco.editor.defineTheme('workbench-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '7a7e85', fontStyle: 'italic' },
      { token: 'keyword', foreground: 'cf8e6d' },
      { token: 'string', foreground: '6aab73' },
      { token: 'number', foreground: '2aacb8' },
      // vs-dark greens hex literals (C's 0xFF, HDL bit strings); CLion keeps one number colour.
      { token: 'number.hex', foreground: '2aacb8' },
      { token: 'type', foreground: '16baac' },
      { token: 'variable', foreground: 'c77dbb' },
      ...semanticRules(dark),
    ],
    colors: {
      ...widgetColors(dark),
      'editor.background': dark.bg,
      'editor.lineHighlightBackground': '#26282e',
      'editorLineNumber.foreground': '#4b5059',
      'editorLineNumber.activeForeground': '#a1a3ab',
      'editorGutter.background': dark.bg,
      'diffEditor.insertedTextBackground': '#3c563026',
      'diffEditor.removedTextBackground': '#6c2c3226',
      'diffEditor.insertedLineBackground': '#29432c55',
      'diffEditor.removedLineBackground': '#4a2a2e55',
    },
  })
  monaco.editor.defineTheme('workbench-light', {
    base: 'vs',
    inherit: true,
    rules: semanticRules(light),
    colors: { ...widgetColors(light), 'editor.background': light.bg, 'editorGutter.background': light.bg },
  })
}
defineThemes()

installEditorKeymap(monaco)

// CLion's Replace is Ctrl+R (Ctrl+H is Type Hierarchy in code files: features/lsp).
monaco.editor.addKeybindingRules([
  { keybinding: monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyR, command: 'editor.action.startFindReplaceAction', when: 'editorFocus' },
])

/**
 * Monaco's own editor context-menu entries that Workbench replaces with CLion-named
 * actions (Go to Declaration or Usages, Find Usages, File Structure, Reformat Code…)
 * or with its own palette. Listing both confused the menu. Reaches into Monaco's menu
 * registry; if a Monaco upgrade changes it, the built-ins simply show again.
 */
const REPLACED_CONTEXT_COMMANDS = new Set([
  'editor.action.revealDefinition',
  'editor.action.revealDeclaration',
  'editor.action.goToTypeDefinition',
  'editor.action.goToImplementation',
  'editor.action.goToReferences',
  'editor.action.quickOutline',
  'editor.action.formatDocument',
  'editor.action.formatSelection',
  'editor.action.quickCommand',
])
try {
  const getMenuItems = MenuRegistry.getMenuItems.bind(MenuRegistry)
  MenuRegistry.getMenuItems = (id) => {
    const items = getMenuItems(id)
    if (id !== MenuId.EditorContext) return items
    return items.filter((i) => i.submenu !== MenuId.EditorContextPeek && !(i.command && REPLACED_CONTEXT_COMMANDS.has(i.command.id)))
  }
} catch {
  // Keep Monaco's menu as it is.
}

/**
 * Context menus: Monaco renders them (fixed position, in a shadow root) inside the
 * focused editor's container. Dock panels are painted in dockview overlays with
 * `contain: paint`, a transform and `isolation`, so a menu there was offset, clipped at
 * the panel's edge and stacked under neighbouring panels. Fixed shadow-DOM context views
 * go to a layer on <body> instead, which carries the theme variables (`monaco-component`).
 * Reaches into Monaco's ContextView; if an upgrade changes it, menus stay in the editor.
 */
const FIXED_SHADOW = 3 // ContextViewDOMPosition.FIXED_SHADOW
let menuLayer: HTMLElement | null = null
function contextMenuLayer() {
  if (!menuLayer) {
    menuLayer = document.createElement('div')
    menuLayer.className = 'monaco-component wb-monaco-layer'
    document.body.appendChild(menuLayer)
  }
  return menuLayer
}
try {
  const setContainer = ContextView.prototype.setContainer
  ContextView.prototype.setContainer = function (container, domPosition) {
    return setContainer.call(this, container && domPosition === FIXED_SHADOW ? contextMenuLayer() : container, domPosition)
  }
} catch {
  // Menus stay inside the editor.
}

export { languageFor } from './languages'
export { monaco }
