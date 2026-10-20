export type Range = [number, number];

export function clampRange(range: Range, duration: number, minimum: number): Range {
  const full = Math.max(duration, minimum);
  const span = Math.min(full, Math.max(minimum, range[1] - range[0]));
  const start = Math.min(Math.max(0, range[0]), full - span);
  return [start, start + span];
}

export function zoomAt(range: Range, anchor: number, factor: number, duration: number, minimum: number): Range {
  const full = Math.max(duration, minimum);
  const span = Math.min(full, Math.max(minimum, (range[1] - range[0]) * factor));
  const fraction = (anchor - range[0]) / (range[1] - range[0]);
  return clampRange([anchor - fraction * span, anchor + (1 - fraction) * span], duration, minimum);
}

export function panFrom(range: Range, deltaPixels: number, width: number, duration: number, minimum: number): Range {
  const delta = deltaPixels / width * (range[1] - range[0]);
  return clampRange([range[0] - delta, range[1] - delta], duration, minimum);
}

export function activeGpi(edges: { mask: number }[]): number[] {
  let changed = 0;
  for (let i = 1; i < edges.length; i++) changed |= edges[i - 1].mask ^ edges[i].mask;
  return [1, 2, 3, 4, 5, 6].filter(n => changed & (1 << (n - 1)));
}
