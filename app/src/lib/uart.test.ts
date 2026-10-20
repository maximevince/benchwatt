import { describe, expect, it } from 'vitest';
import { linesFromChunks, filterLines, linesInRange, lineStartForChunk } from './uart';
describe('UART console mapping', () => {
  const chunks = [
    { dir: 'rx' as const, t: 0.1, bytes: [104,101,108,108,111,13] },
    { dir: 'tx' as const, t: 0.1005, bytes: [120,10] },
    { dir: 'rx' as const, t: 0.101, bytes: [32,119,111,114,108,100,10] },
  ];
  it('joins split RX with first-byte time and interleaves TX', () => {
    expect(linesFromChunks(chunks).map(x => [x.dir,x.t,x.text])).toEqual([['rx',0.1,'hello world'],['tx',0.1005,'x']]);
  });
  it('filters without case sensitivity and maps selected times', () => {
    const lines = linesFromChunks(chunks);
    expect(filterLines(lines,'WORLD').map(x => x.text)).toEqual(['hello world']);
    expect(linesInRange(lines,[0.1001,0.101]).map(x => x.dir)).toEqual(['tx']);
    expect(lineStartForChunk(chunks,2)).toBe(0.1);
  });
});
