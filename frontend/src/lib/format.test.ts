import { describe, expect, it } from 'vitest'
import { bytesLabel, friendlyError } from './format'
describe('format helpers', () => {
  it('formats transfer sizes', () => {
    expect(bytesLabel(1024)).toBe('1.0 KB')
    expect(bytesLabel(1048576)).toBe('1.00 MB')
  })
  it('handles backend exceptions', () => {
    expect(friendlyError(new Error('Bad document'))).toBe('Bad document')
  })
})
