import { describe, expect, it } from 'vitest';
import { decodeView } from './view';

describe('recording view IPC', () => {
  it('decodes packed raw values from a byte-offset slice', () => {
    const storage = new Uint8Array(19);
    const data = new DataView(storage.buffer, 2);
    data.setUint8(0, 0); data.setUint32(1, 1, true);
    data.setFloat64(5, 1.25, true); data.setFloat32(13, 0.0184, true);
    const view = decodeView(storage.subarray(2));
    expect(view.kind).toBe('raw');
    if (view.kind === 'raw') { expect(view.t).toEqual([1.25]); expect(view.v[0]).toBeCloseTo(0.0184); }
  });
  it('decodes min/max and rejects malformed payloads', () => {
    const bytes = new Uint8Array(21), data = new DataView(bytes.buffer);
    data.setUint8(0, 1); data.setUint32(1, 1, true);
    data.setFloat64(5, 2.5, true); data.setFloat32(13, -1, true); data.setFloat32(17, 3, true);
    expect(decodeView(bytes)).toEqual({ kind: 'envelope', t: [2.5], min: [-1], max: [3] });
    expect(() => decodeView(bytes.subarray(0, 20))).toThrow('invalid view');
    expect(() => decodeView(new Uint8Array([2, 0, 0, 0, 0]))).toThrow('invalid view');
  });
});
