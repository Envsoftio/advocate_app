import { describe, expect, it, vi } from 'vitest'
vi.mock('./api',()=>({api:vi.fn(),saveReaderPosition:vi.fn(),ApiError:class extends Error {constructor(public status:number,message:string){super(message)}}}))
import { api, saveReaderPosition } from './api'
import { nextZoom, pageAtOffset, readerScale, recalledReadingPosition, rememberReadingPosition, loadReadingPosition, flushReadingPositions } from './pdfReader'
describe('reader geometry and session positions', () => {
  it('keeps numeric magnification independent of panel width and fits both dimensions', () => {
    expect(readerScale('100', 800, 600, 600, 800)).toBeCloseTo(4/3)
    expect(readerScale('100', 1200, 900, 600, 800)).toBeCloseTo(4/3)
    expect(readerScale('fit-width', 640, 500, 600, 800)).toBe(1)
    expect(readerScale('fit-page', 640, 440, 600, 800)).toBe(.5)
    expect(nextZoom('fit-width', 4/3, 1)).toBe('125')
  })
  it('resolves mixed-height page offsets including boundaries', () => {
    const offsets = [0,800,1300,2200]
    expect(pageAtOffset(offsets,799)).toBe(0)
    expect(pageAtOffset(offsets,800)).toBe(1)
    expect(pageAtOffset(offsets,9000)).toBe(2)
  })
  it('separates revisions and copies state before storing', () => {
    const state = {page:3,y:.25,x:0,zoom:'125',mode:'continuous' as const}
    rememberReadingPosition('synthetic:r1',state); state.page=9
    expect(recalledReadingPosition('synthetic:r1')?.page).toBe(3)
    expect(recalledReadingPosition('synthetic:r2')).toBeUndefined()
  })
  it('validates persisted state and keeps a newer local position during a delayed read', async()=>{
    const key='a'.repeat(32)+':1'
    let resolve!:(value:unknown)=>void
    vi.mocked(api).mockImplementationOnce(()=>new Promise(done=>{resolve=done}) as any)
    const loading=loadReadingPosition(key)
    rememberReadingPosition(key,{page:8,y:.4,x:0,zoom:'125',mode:'continuous'})
    resolve({page:2,y:0,x:0,zoom:'100',mode:'single'})
    expect((await loading)?.page).toBe(8)
    await flushReadingPositions()
    vi.mocked(api).mockResolvedValueOnce({page:-1,y:0,x:0,zoom:'100',mode:'single'})
    expect(await loadReadingPosition('b'.repeat(32)+':1')).toBeUndefined()
  })
  it('serializes a final save behind an in-flight save without dropping the newer position',async()=>{
    const key='c'.repeat(32)+':1'
    let finish!:()=>void
    vi.mocked(saveReaderPosition).mockImplementationOnce(()=>new Promise<void>(done=>{finish=done}))
    rememberReadingPosition(key,{page:1,y:0,x:0,zoom:'100',mode:'single'})
    const first=flushReadingPositions()
    rememberReadingPosition(key,{page:9,y:.25,x:0,zoom:'150',mode:'continuous'})
    const second=flushReadingPositions()
    finish();await Promise.all([first,second])
    const calls=vi.mocked(saveReaderPosition).mock.calls
    expect(calls[calls.length-1]).toEqual([key,{page:9,y:.25,x:0,zoom:'150',mode:'continuous'}])
  })
  it('retains a failed save for retry without requiring the reading position to change',async()=>{
    const key='d'.repeat(32)+':1'
    const state={page:7,y:0,x:0,zoom:'125',mode:'single' as const}
    vi.mocked(saveReaderPosition).mockRejectedValueOnce(new Error('Synthetic unavailable service'))
    rememberReadingPosition(key,state)
    await flushReadingPositions()
    const count=vi.mocked(saveReaderPosition).mock.calls.length
    rememberReadingPosition(key,state)
    await flushReadingPositions()
    expect(vi.mocked(saveReaderPosition).mock.calls.length).toBe(count+1)
    expect(vi.mocked(saveReaderPosition).mock.lastCall).toEqual([key,state])
  })
})
