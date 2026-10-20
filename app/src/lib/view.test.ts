import { describe, expect, it } from 'vitest';
import { decodeView, envelopeRuns } from './view';

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

describe('recording envelope geometry', () => {
  it.each([2, 4, 5, 10])('keeps 20 ms pulses connected at %i s across bucket sizes and zooms', duration => {
    const rate = 4000, samples = Array.from({ length: duration * rate }, (_, i) => i / rate % 1 < 0.02 ? 0.03 : 0.00005);
    for (const bucketSize of [16, 256]) {
      const t: number[] = [], min: number[] = [], max: number[] = [];
      for (let start = 0; start < samples.length; start += bucketSize) {
        const bucket = samples.slice(start, start + bucketSize);
        t.push(start / rate); min.push(Math.min(...bucket)); max.push(Math.max(...bucket));
      }
      for (const range of [[0, duration], [0.9, 1.1]] as [number, number][]) {
        const first = t.findIndex(time => time >= range[0]);
        const last = t.findIndex(time => time >= range[1]);
        const end = last < 0 ? t.length : last;
        const visible = { kind: 'envelope' as const, t: t.slice(first, end), min: min.slice(first, end), max: max.slice(first, end) };
        const runs = envelopeRuns(visible, range, 1126);
        expect(runs).toHaveLength(1);
        const { upper, lower, baseline } = runs[0];
        expect(upper.length).toBe(visible.t.length * 2);
        expect(baseline).toBe(0.00005);
        expect(upper.some(point => point.value === 0.03)).toBe(true);
        expect(upper.some(point => point.value === 0.00005)).toBe(true);
        const filledArea = upper.reduce((area, point, i) => i % 2 ? area + (point.x - upper[i - 1].x) * (point.value - baseline) : area, 0);
        expect(filledArea).toBeGreaterThan(0);
        for (let i = 2; i < upper.length; i += 2) {
          expect(upper[i].x).toBeCloseTo(upper[i - 1].x);
          expect(lower[i].x).toBeCloseTo(lower[i - 1].x);
        }
        // A flat high bucket still has a vertical edge to the neighboring low bucket.
        expect(upper.some((point, i) => i > 0 && point.x === upper[i - 1].x && point.value !== upper[i - 1].value)).toBe(true);
      }
    }
  });

  it('breaks only at invalid buckets', () => {
    const runs = envelopeRuns({ kind: 'envelope', t: [0, 1, 2, 3], min: [0, 0, NaN, 0], max: [0, 1, NaN, 0] }, [0, 4], 400);
    expect(runs).toHaveLength(2);
    expect(runs[0].upper.at(-1)?.x).toBe(200);
    expect(runs[1].upper[0].x).toBe(300);
  });
});
