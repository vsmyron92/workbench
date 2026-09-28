// The same palette as tokens.css, for libraries configured from JavaScript
// (Monaco themes, xterm themes). Read the live CSS variables so light/dark stay in sync.

export function cssVar(name: string, fallback = ''): string {
  if (typeof document === 'undefined') return fallback
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return v || fallback
}

export function currentTheme(): 'dark' | 'light' {
  return document.documentElement.dataset.theme === 'light' ? 'light' : 'dark'
}

/** xterm.js ITheme for the current theme. */
export function xtermTheme() {
  const dark = currentTheme() === 'dark'
  return {
    background: cssVar('--bg', dark ? '#1e1f22' : '#ffffff'),
    foreground: cssVar('--fg', dark ? '#dfe1e5' : '#1e1f22'),
    cursor: cssVar('--fg', dark ? '#dfe1e5' : '#1e1f22'),
    cursorAccent: cssVar('--bg', '#1e1f22'),
    selectionBackground: dark ? 'rgba(84, 138, 247, 0.35)' : 'rgba(53, 116, 240, 0.25)',
    black: dark ? '#1e1f22' : '#000000',
    red: dark ? '#f0616e' : '#c7222d',
    green: dark ? '#6fc27a' : '#208a3c',
    yellow: dark ? '#e5c06a' : '#a46704',
    blue: dark ? '#5e98f8' : '#3574f0',
    magenta: dark ? '#c78ae6' : '#9744c7',
    cyan: dark ? '#4fc1cf' : '#0e7fa8',
    white: dark ? '#dfe1e5' : '#5a5d63',
    brightBlack: dark ? '#6f737a' : '#818594',
    brightRed: dark ? '#ff7b86' : '#e55765',
    brightGreen: dark ? '#8ad993' : '#34a853',
    brightYellow: dark ? '#f2d38a' : '#c78300',
    brightBlue: dark ? '#82b1ff' : '#548af7',
    brightMagenta: dark ? '#dca6f5' : '#b060e0',
    brightCyan: dark ? '#77d8e3' : '#1597c4',
    brightWhite: dark ? '#ffffff' : '#1e1f22',
  }
}

/** Name of the Monaco theme registered by features/files (and used by every Monaco instance). */
export function monacoThemeName(): string {
  return currentTheme() === 'dark' ? 'workbench-dark' : 'workbench-light'
}
