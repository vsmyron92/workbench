import { describe, expect, it } from 'vitest'
import { joinPath } from './FolderBrowser'

describe('joinPath', () => {
  it('uses the separator the folder already has', () => {
    expect(joinPath('/home/ada', 'src')).toBe('/home/ada/src')
    expect(joinPath('/', 'etc')).toBe('/etc')
    expect(joinPath('C:\\Users\\ada', 'src')).toBe('C:\\Users\\ada\\src')
    expect(joinPath('C:\\', 'src')).toBe('C:\\src')
    expect(joinPath('C:/Users', 'src')).toBe('C:/Users/src')
  })
})
