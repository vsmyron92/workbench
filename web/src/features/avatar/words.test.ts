// The words of the status bar and the palette: what the contract names (10.6), the same
// words as the views use for the same things, and names that stay short enough to leave
// room for what an entry does.

import { describe, expect, it } from 'vitest'
import { WORDS, calledAs, replyTo } from './Bubble'
import { MENU_WORDS } from './menu'
import { COMMAND_WORDS, MAX_ENTRY_CHARS, MAX_TITLE_CHARS, STATUS_WORDS, called, cut, giveEntry, moreSessions, notTold, replyCommand, sessionName, shownTitle, turnOffEntry } from './words'

describe('the words of the status bar and the palette', () => {
  it('names the widget’s menu as 10.6 does, and the entries an avatar’s own menu also has in its words', () => {
    expect([STATUS_WORDS.stop, STATUS_WORDS.voice, STATUS_WORDS.show, STATUS_WORDS.settings]).toEqual(['Stop speaking', 'Voice in this browser', 'Show avatars', 'Avatar settings'])
    expect(STATUS_WORDS.stop).toBe(MENU_WORDS.stop)
    expect(STATUS_WORDS.voice).toBe(MENU_WORDS.voice)
    expect(STATUS_WORDS.settings).toBe(MENU_WORDS.settings)
  })

  it('asks about online voices with the sentence of rule 7, as the bubble does', () => {
    expect(STATUS_WORDS.online).toBe('What an avatar says is sent to the browser maker’s speech service to be spoken.')
    expect(STATUS_WORDS.online).toBe(WORDS.online)
    expect(STATUS_WORDS.allowOnline).toBe(WORDS.allowOnline)
    expect(STATUS_WORDS.noVoices).toBe(WORDS.noVoices)
    expect(STATUS_WORDS.askOnline).toBe('Allow online voices?')
  })

  it('names the palette commands as 10.6 does', () => {
    expect([COMMAND_WORDS.show, COMMAND_WORDS.voice, COMMAND_WORDS.stop, COMMAND_WORDS.give, COMMAND_WORDS.turnOff, COMMAND_WORDS.settings]).toEqual([
      'Avatar: Show or Hide Avatars',
      'Avatar: Voice On or Off',
      'Avatar: Stop Speaking',
      'Avatar: Give This Session an Avatar',
      'Avatar: Turn Off for This Session',
      'Avatar Settings',
    ])
    expect(replyCommand('Pip')).toBe('Avatar: Reply to Pip…')
    // A name that was cut ends in an ellipsis already: the title has one, not two.
    expect(replyCommand('A very long session title that goes…')).toBe('Avatar: Reply to A very long session title that goes…')
    // The same name as the reply box's placeholder.
    expect(replyCommand(called('', 'Fix the flaky upload test'))).toBe(`Avatar: ${replyTo(calledAs('', 'Fix the flaky upload test'))}`)
  })

  it('calls an avatar what the bubble calls it: its own name, else the session’s title, cut to a few words', () => {
    const long = 'Refactor the importer so that it can take the new export format of the warehouse'
    const cases: [string, string][] = [
      ['Pip', 'Fix the flaky upload test'],
      ['', 'Fix the flaky upload test'],
      ['', long],
      [long, 'short'],
      ['', ''],
      ['👩‍💻 Ada', ''],
      ['', '😀'.repeat(40)],
      ['x'.repeat(32), ''],
      ['x'.repeat(33), ''],
    ]
    for (const [name, title] of cases) expect(called(name, title), `${name}|${title}`).toBe(calledAs(name, title))
    expect(called('', long)).toBe('Refactor the importer so that i…')
    expect(Array.from(called('', long))).toHaveLength(32)
  })

  it('cuts a text by characters, never inside one, and leaves a short one alone', () => {
    expect(cut('abc', 3)).toBe('abc')
    expect(cut('abcd', 3)).toBe('ab…')
    expect(cut('ab cd', 4)).toBe('ab…')
    expect(cut('😀😀😀😀', 3)).toBe('😀😀…')
    expect(cut('', 5)).toBe('')
    expect(cut(undefined as unknown as string, 5)).toBe('')
    expect(cut('abc', 0)).toBe('…')
  })

  it('names a session in a menu entry so that what the entry does stays in sight', () => {
    expect(sessionName('Fix the tests')).toBe('Fix the tests')
    expect(sessionName('  ')).toBe('Untitled session')
    expect(sessionName(undefined as unknown as string)).toBe('Untitled session')
    const long = 'Refactor the importer so that it can take the new export format'
    expect(Array.from(sessionName(long))).toHaveLength(MAX_TITLE_CHARS)
    expect(sessionName(long).endsWith('…')).toBe(true)
    expect(sessionName(long, 10)).toBe('Refactor…')
    expect(giveEntry('Fix the tests', false)).toBe('Fix the tests: give an avatar')
    expect(giveEntry('Fix the tests', true)).toBe('Fix the tests: give an avatar (asked)')
    expect(turnOffEntry('Fix the tests')).toBe('Fix the tests: turn avatar off')
    // A title that fits is not cut: the menu host has room for 45 characters or so (312 px of text).
    expect(giveEntry('Fix the flaky upload test', false)).toBe('Fix the flaky upload test: give an avatar')
    expect(turnOffEntry('Write the release notes')).toBe('Write the release notes: turn avatar off')
    // A long one gets the room that what the entry does leaves: the entry is never longer, and ends in what it does.
    expect(giveEntry(long, false)).toBe('Refactor the importer so tha…: give an avatar')
    expect(giveEntry(long, true)).toBe('Refactor the importe…: give an avatar (asked)')
    expect(turnOffEntry(long)).toBe('Refactor the importer so th…: turn avatar off')
    for (const label of [giveEntry(long, false), giveEntry(long, true), turnOffEntry(long), giveEntry('😀'.repeat(80), true)]) expect(Array.from(label)).toHaveLength(MAX_ENTRY_CHARS)
    expect([moreSessions(1), moreSessions(7)]).toEqual(['1 more session', '7 more sessions'])
  })

  it('says in the tooltip how many avatars are shown, whether they speak, and who asked for one', () => {
    expect(shownTitle(0, false, 0)).toBe('No avatar shown · voice off')
    expect(shownTitle(1, true, 0)).toBe('1 avatar shown · voice on')
    expect(shownTitle(3, false, 0)).toBe('3 avatars shown · voice off')
    expect(shownTitle(2, true, 1)).toBe('2 avatars shown · voice on · 1 session asks for one')
    expect(shownTitle(0, false, 4)).toBe('No avatar shown · voice off · 4 sessions ask for one')
    expect(shownTitle(-1, false, -2)).toBe('No avatar shown · voice off')
    expect(STATUS_WORDS.hiddenTitle).toBe('Avatars are hidden in this browser: nothing is shown or said here')
    expect(STATUS_WORDS.hidden).toBe('hidden')
  })

  it('says that a session could not be told of its avatar, and what happens next', () => {
    expect(notTold('Fix the tests')).toBe('Fix the tests has an avatar, but the session could not be told now. Workbench tells it when it is next idle.')
    expect(notTold('x'.repeat(100))).toContain('… has an avatar')
  })

  it('is plain: no exclamation mark, no dash for a pause, no markup', () => {
    const all = [...Object.values(STATUS_WORDS), ...Object.values(COMMAND_WORDS), replyCommand('Pip'), giveEntry('a', true), turnOffEntry('a'), moreSessions(2), shownTitle(2, true, 2), notTold('a')]
    for (const text of all) {
      expect(text, text).not.toMatch(/[!—–<>*_`]/)
      expect(text.trim(), text).toBe(text)
    }
  })
})
