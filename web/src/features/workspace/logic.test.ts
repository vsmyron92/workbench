import { describe, expect, it } from 'vitest'
import type { WorkspaceCard } from './api'
import {
  askPrompt,
  externalLinkTarget,
  fileUrl,
  groupCards,
  headingSlug,
  isProjectless,
  listedScope,
  manifestMode,
  relativeDay,
  repoLinkPath,
  resolveRelative,
  resolveStep,
  singleModelManifest,
  visibleCards,
} from './logic'
import { draftKey, isDirty, useWsDrafts } from './store'

function card(id: string, over: Partial<WorkspaceCard> = {}): WorkspaceCard {
  return {
    id,
    scope: 'proj',
    scopeName: 'Project',
    origin: 'workbench',
    title: id,
    description: '',
    type: 'standalone',
    category: 'research',
    created: '2026-09-20',
    folder: `2026-09-20_${id}`,
    folderPath: `/data/workspace/proj/2026-09-20_${id}`,
    steps: [],
    status: 'active',
    pinned: false,
    sample: false,
    archived: false,
    touchedAt: 1000,
    editable: true,
    base: '/view/g/f/',
    ...over,
  }
}

describe('card lists', () => {
  const cards = [
    card('old', { archived: true, touchedAt: 10, description: 'load curves' }),
    card('pinned', { pinned: true, touchedAt: 5 }),
    card('fresh', { touchedAt: 900, category: 'design' }),
    card('newest', { touchedAt: 999 }),
  ]

  it('hides the archive unless searching or asked', () => {
    expect(visibleCards(cards, '', false).map((c) => c.id)).toEqual(['pinned', 'fresh', 'newest'])
    expect(visibleCards(cards, '', true)).toHaveLength(4)
    expect(visibleCards(cards, 'LOAD', false).map((c) => c.id)).toEqual(['old'])
  })

  it('groups pinned first, then categories by freshness; flat when searching', () => {
    const g = groupCards(visibleCards(cards, '', false), false)
    expect(g.pinned.map((c) => c.id)).toEqual(['pinned'])
    expect(g.groups.map((x) => [x.key, x.cards.map((c) => c.id)])).toEqual([
      ['research', ['newest']],
      ['design', ['fresh']],
    ])
    const flat = groupCards(cards, true)
    expect(flat.groups).toHaveLength(1)
    expect(flat.groups[0].cards.map((c) => c.id)).toEqual(['newest', 'fresh', 'old'])
  })
})

describe('steps and paths', () => {
  it('resolves the step to show', () => {
    expect(resolveStep(3, 1, 0)).toBe(1)
    expect(resolveStep(3, 7, 0)).toBe(0)
    expect(resolveStep(3, undefined, undefined)).toBe(2)
    expect(resolveStep(3, -1, 9)).toBe(2)
    expect(resolveStep(0, 0, 0)).toBe(-1)
  })

  it('builds encoded grant URLs', () => {
    expect(fileUrl('/view/g/f/', 'img/a b#1.png')).toBe('/view/g/f/img/a%20b%231.png')
    expect(fileUrl('/view/g/f/', 'r.html', 42)).toBe('/view/g/f/r.html?v=42')
  })

  it('resolves references inside the card only', () => {
    expect(resolveRelative('docs/a.md', 'img/x.png')).toBe('docs/img/x.png')
    expect(resolveRelative('docs/a.md', '../b.md#top')).toBe('b.md')
    expect(resolveRelative('docs/a.md', './c%20d.png?v=1')).toBe('docs/c d.png')
    expect(resolveRelative('a.md', '../outside.md')).toBeNull()
    expect(resolveRelative('a.md', 'https://example.com/x.png')).toBeNull()
    expect(resolveRelative('a.md', 'data:image/png;base64,xx')).toBeNull()
    expect(resolveRelative('a.md', '#section')).toBeNull()
    expect(resolveRelative('a.md', '/abs')).toBeNull()
  })

  it('wraps a single model as a manifest', () => {
    const m = singleModelManifest('models/fox.glb', 'Fox')
    expect(m.tests[0].models[0]).toEqual({ file: 'fox.glb', label: 'Fox' })
  })
})

describe('text', () => {
  it('says relative days', () => {
    const now = new Date(2026, 8, 26, 15, 0).getTime()
    expect(relativeDay(now - 30_000, now)).toBe('just now')
    expect(relativeDay(now - 5 * 60_000, now)).toBe('5 min ago')
    expect(relativeDay(new Date(2026, 8, 26, 1, 0).getTime(), now)).toBe('today')
    expect(relativeDay(new Date(2026, 8, 25, 23, 0).getTime(), now)).toBe('yesterday')
    expect(relativeDay(new Date(2026, 8, 20).getTime(), now)).toBe('6 d ago')
    expect(relativeDay(0, now)).toBe('')
  })

  it('asks the agent with the card context', () => {
    const c = card('r', { title: 'Load report', steps: [{ index: 0, name: 'Summary', path: 'r.html', kind: 'html', exists: true }] })
    const p = askPrompt(c, c.steps[0])
    expect(p).toContain('"Load report" (cardId r, scope proj)')
    expect(p).toContain('/data/workspace/proj/2026-09-20_r')
    expect(p).toContain('0. Summary (r.html)')
    expect(p.endsWith('Question: ')).toBe(true)
  })
})

describe('links', () => {
  const here = { origin: 'http://127.0.0.1:7960', hostname: '127.0.0.1', port: '7960', protocol: 'http:' }

  it('opens only external http(s) links a report asks for', () => {
    expect(externalLinkTarget('https://example.com/a?b#c', here)).toBe('https://example.com/a?b#c')
    expect(externalLinkTarget('http://127.0.0.1:8080/', here)).toBe('http://127.0.0.1:8080/')
    for (const bad of [
      'http://127.0.0.1:7960/#wbk=x',
      'http://localhost:7960/',
      'http://[::1]:7960/',
      'javascript:alert(1)',
      'data:text/html,x',
      'blob:http://127.0.0.1:7960/x',
      'file:///etc/passwd',
      'not a url',
      42,
    ]) {
      expect(externalLinkTarget(bad, here)).toBeNull()
    }
    const lan = { origin: 'http://192.168.1.5:7960', hostname: '192.168.1.5', port: '7960', protocol: 'http:' }
    expect(externalLinkTarget('http://127.0.0.1:7960/', lan)).toBeNull()
    expect(externalLinkTarget('http://192.168.1.5:7960/x', lan)).toBeNull()
    expect(externalLinkTarget('http://192.168.1.6:7960/x', lan)).toBe('http://192.168.1.6:7960/x')
  })

  it('resolves links that leave a repository card against its project', () => {
    const repo = card('g', { origin: 'repo', folder: 'make-workspace-yours' })
    expect(repoLinkPath(repo, 'get-started.md', '../../knowledge/voice-dictation.md')).toBe('knowledge/voice-dictation.md')
    expect(repoLinkPath(repo, 'docs/a.md', '../../b.md')).toBe('workspace/b.md')
    expect(repoLinkPath(repo, 'a.md', '/README.md')).toBe('README.md')
    expect(repoLinkPath(repo, 'a.md', '../../../outside.md')).toBeNull()
    expect(repoLinkPath(repo, 'a.md', 'https://x.dev')).toBeNull()
    expect(repoLinkPath(repo, 'a.md', '#top')).toBeNull()
    expect(repoLinkPath(card('w'), 'a.md', '../../x.md')).toBeNull()
  })

  it('makes heading anchors like GitHub', () => {
    expect(headingSlug('Get started!')).toBe('get-started')
    expect(headingSlug(' Step 2: the “plan” ')).toBe('step-2-the-plan')
  })
})

describe('3D manifests', () => {
  it("maps Mr. Mak's quads to the wireframe and drops unknown modes", () => {
    expect(manifestMode('quads')).toBe('wire')
    expect(manifestMode('pbr')).toBe('pbr')
    expect(manifestMode('sparkles')).toBeUndefined()
    expect(manifestMode(3)).toBeUndefined()
  })
})

describe('markdown drafts', () => {
  it('keeps a draft per file and tracks saves', () => {
    const s = useWsDrafts.getState()
    const k = draftKey('home', 'c', 'notes.md')
    s.put(k, { text: 'a', base: 'r1', baseText: 'a' })
    expect(isDirty(useWsDrafts.getState().drafts[k])).toBe(false)
    s.setText(k, 'a UNSAVED')
    expect(isDirty(useWsDrafts.getState().drafts[k])).toBe(true)
    // Saving "a UNSAVED" while more was typed keeps the newer text, still dirty.
    s.setText(k, 'a UNSAVED more')
    s.saved(k, 'r2', 'a UNSAVED')
    expect(useWsDrafts.getState().drafts[k]).toEqual({ text: 'a UNSAVED more', base: 'r2', baseText: 'a UNSAVED' })
    s.put(draftKey('home', 'c', 'other.md'), { text: 'x', base: 'r', baseText: 'x' })
    s.put(draftKey('home', 'd', 'notes.md'), { text: 'x', base: 'r', baseText: 'x' })
    s.dropCard('home', 'c')
    expect(Object.keys(useWsDrafts.getState().drafts)).toEqual([draftKey('home', 'd', 'notes.md')])
  })
})

describe('scopes', () => {
  it('lists the Sandbox on request, else the project or Home', () => {
    expect(listedScope('sandbox', 'shop')).toBe('wb-sandbox')
    expect(listedScope('sandbox', null)).toBe('wb-sandbox')
    expect(listedScope('project', 'shop')).toBe('shop')
    expect(listedScope('project', null)).toBe('home')
    expect(listedScope('home', 'shop')).toBe('home')
  })
  it('knows which scopes belong to no project', () => {
    expect([isProjectless('home'), isProjectless('wb-sandbox'), isProjectless('shop')]).toEqual([true, true, false])
  })
})

describe('draft cleanup', () => {
  it('dropScope forgets every draft of one scope only', () => {
    const s = useWsDrafts.getState()
    s.put(draftKey('wb-sandbox', 'a', 'x.md'), { text: 'x', base: 'r', baseText: 'x' })
    s.put(draftKey('wb-sandbox', 'b', 'y.md'), { text: 'y', base: 'r', baseText: 'y' })
    s.put(draftKey('home', 'a', 'x.md'), { text: 'x', base: 'r', baseText: 'x' })
    s.dropScope('wb-sandbox')
    const keys = Object.keys(useWsDrafts.getState().drafts)
    expect(keys).toContain(draftKey('home', 'a', 'x.md'))
    expect(keys.filter((k) => k.startsWith('wb-sandbox\n'))).toEqual([])
  })
})
