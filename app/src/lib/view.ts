export type View = { kind: 'raw'; t: number[]; v: number[] } | { kind: 'envelope'; t: number[]; min: number[]; max: number[] };

/** Decode the packed little-endian recording response. */
export function decodeView(bytes: Uint8Array): View {
  if (bytes.length < 5) throw new Error('short view');
  const data = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const mode = data.getUint8(0), count = data.getUint32(1, true), stride = mode === 0 ? 12 : 16;
  if (mode > 1 || bytes.length !== 5 + count * stride) throw new Error('invalid view');
  const t: number[] = [], v: number[] = [], min: number[] = [], max: number[] = [];
  for (let i = 0, at = 5; i < count; i++, at += stride) {
    t.push(data.getFloat64(at, true));
    if (mode === 0) v.push(data.getFloat32(at + 8, true));
    else { min.push(data.getFloat32(at + 8, true)); max.push(data.getFloat32(at + 12, true)); }
  }
  return mode === 0 ? { kind: 'raw', t, v } : { kind: 'envelope', t, min, max };
}
