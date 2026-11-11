export type Range = [number, number];

/** Return evenly spaced ticks using a 1-2-5 step, without exceeding maxCount. */
export function niceTicks(min: number, max: number, maxCount: number): number[] {
  if (!Number.isFinite(min) || !Number.isFinite(max) || maxCount < 1) return [];
  if (min === max) return [min];
  if (min > max) [min, max] = [max, min];
  if (maxCount === 1) return [min];

  const rawStep = (max - min) / (maxCount - 1);
  const exponent = Math.floor(Math.log10(rawStep));
  const magnitude = 10 ** exponent;
  const fraction = rawStep / magnitude;
  const niceFraction = fraction <= 1 ? 1 : fraction <= 2 ? 2 : fraction <= 5 ? 5 : 10;
  const step = niceFraction * magnitude;
  const first = Math.floor(min / step + 1e-12);
  const last = Math.ceil(max / step - 1e-12);
  const decimals = Math.max(0, -exponent + 2);
  const round = (value: number) => Number(value.toFixed(decimals));

  return Array.from({ length: Math.max(0, last - first + 1) }, (_, i) => round((first + i) * step));
}

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
