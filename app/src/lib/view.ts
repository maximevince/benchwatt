export type View = { kind: 'raw'; t: number[]; v: number[] } | { kind: 'envelope'; t: number[]; min: number[]; max: number[] };

export type EnvelopePoint = { x: number; value: number };
export type EnvelopeRun = { upper: EnvelopePoint[]; lower: EnvelopePoint[]; baseline: number };

/** Build connected bucket boundaries. Invalid buckets split the trace into separate runs. */
export function envelopeRuns(view: Extract<View, { kind: 'envelope' }>, range: [number, number], width: number): EnvelopeRun[] {
  const runs: EnvelopeRun[] = [];
  const project = (t: number) => (t - range[0]) / (range[1] - range[0]) * width;
  let run: EnvelopeRun | null = null;
  for (let i = 0; i < view.t.length; i++) {
    const start = view.t[i];
    const end = i + 1 < view.t.length ? view.t[i + 1] :
      Math.min(range[1], start + (i > 0 ? start - view.t[i - 1] : range[1] - range[0]));
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start ||
        !Number.isFinite(view.min[i]) || !Number.isFinite(view.max[i])) {
      run = null;
      continue;
    }
    if (!run) { run = { upper: [], lower: [], baseline: view.min[i] }; runs.push(run); }
    run.baseline = Math.min(run.baseline, view.min[i]);
    const left = project(start), right = project(end);
    run.upper.push({ x: left, value: view.max[i] }, { x: right, value: view.max[i] });
    run.lower.push({ x: left, value: view.min[i] }, { x: right, value: view.min[i] });
  }
  return runs;
}

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
