import { describe, expect, it } from 'vitest'
import { fileSize, parseLabels } from '../links'
import { freeName, uploadName } from './uploads'

describe('attachment names', () => {
  const at = new Date(2026, 8, 27, 14, 5, 9)

  it('keeps real names and stamps clipboard images', () => {
    expect(uploadName({ name: 'diagram v2.png', type: 'image/png' }, at)).toBe('diagram v2.png')
    expect(uploadName({ name: 'image.png', type: 'image/png' }, at)).toBe('image-20260927-140509.png')
    expect(uploadName({ name: '', type: 'image/jpeg' }, at)).toBe('image-20260927-140509.jpg')
    expect(uploadName({ name: 'a/b.txt', type: 'text/plain' }, at)).toBe('a_b.txt')
  })

  it('finds a free name next to existing attachments', () => {
    expect(freeName('a.png', ['b.png'])).toBe('a.png')
    expect(freeName('a.png', ['a.png', 'a-1.png'])).toBe('a-2.png')
    expect(freeName('README', ['README'])).toBe('README-1')
    expect(freeName('.env', ['.env'])).toBe('.env-1')
  })

  it('parses typed labels like Confluence stores them', () => {
    expect(parseLabels(' Review, v2  draft,,review ')).toEqual(['review', 'v2', 'draft'])
    expect(parseLabels('   ')).toEqual([])
  })

  it('formats sizes', () => {
    expect(fileSize(512)).toBe('512 B')
    expect(fileSize(2048)).toBe('2.0 KB')
    expect(fileSize(300 * 1024)).toBe('300 KB')
    expect(fileSize(5 * 1024 * 1024)).toBe('5.0 MB')
    expect(fileSize(null)).toBe('')
  })
})
