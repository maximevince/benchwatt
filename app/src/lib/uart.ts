import type { UartChunk } from './backend';
export type UartLine = { id: string; dir: 'rx' | 'tx'; t: number; text: string; bytes: number[] };
export function linesFromChunks(chunks: UartChunk[]): UartLine[] {
  const ordered = [...chunks].sort((a,b) => a.t-b.t);
  const lines: UartLine[] = [];
  let rx: { t: number; bytes: number[] } | null = null;
  const push = (dir: 'rx' | 'tx', t: number, bytes: number[]) => {
    lines.push({ id: `${dir}:${t}:${lines.length}`, dir, t, bytes, text: new TextDecoder().decode(new Uint8Array(bytes)).replaceAll('\r', '') });
  };
  for (const chunk of ordered) {
    if (chunk.dir === 'tx') {
      for (const part of splitBytes(chunk.bytes)) push('tx',chunk.t,part);
      continue;
    }
    for (const byte of chunk.bytes) {
      rx ??= { t: chunk.t, bytes: [] };
      if (byte !== 13 && byte !== 10) rx.bytes.push(byte);
      if (byte === 10) { push('rx',rx.t,rx.bytes); rx = null; }
    }
  }
  if (rx) push('rx',rx.t,rx.bytes);
  return lines.sort((a,b) => a.t-b.t);
}
function splitBytes(bytes: number[]): number[][] {
  const result: number[][] = []; let part: number[] = [];
  for (const byte of bytes) { if (byte !== 13 && byte !== 10) part.push(byte); if (byte === 10) { result.push(part); part=[]; } }
  if (part.length) result.push(part);
  return result;
}
export function filterLines(lines: UartLine[], query: string, showTx = true): UartLine[] {
  const needle = query.toLocaleLowerCase();
  return lines.filter(line => (showTx || line.dir === 'rx') && line.text.toLocaleLowerCase().includes(needle));
}
export function linesInRange(lines: UartLine[], range: [number, number] | null): UartLine[] {
  return range ? lines.filter(line => line.t >= range[0] && line.t <= range[1]) : [];
}
export function formatUartTime(t: number, span = 1): string {
  return span < .01 ? `${(t*1e6).toFixed(0)} µs` : span < 1 ? `${(t*1e3).toFixed(3)} ms` : span < 120 ? `${t.toFixed(6)} s` : `${(t/60).toFixed(4)} min`;
}

/** Time of the first byte of the line represented by a plotted UART chunk. */
export function lineStartForChunk(chunks: UartChunk[], index: number): number | null {
  const chunk=chunks[index];
  if (!chunk) return null;
  if (chunk.dir === 'tx') return chunk.t;
  let start=chunk.t;
  for (let i=index-1; i>=0; i--) {
    const prior=chunks[i];
    if (prior.dir !== 'rx') continue;
    if (prior.bytes.at(-1) === 10) break;
    start=prior.t;
  }
  return start;
}
