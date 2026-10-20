import type { View } from './view';

/** Values from the currently loaded plot buckets, at the bucket nearest to time. */
export function valuesAtTimeTsv(time: number, channels: string[], views: Map<string, View>): string {
  const rows = ['channel\tvalue', `time\t${time.toFixed(6)} s`];
  for (const ch of channels) {
    const view = views.get(ch);
    if (!view?.t.length) { rows.push(`${ch}\t—`); continue; }
    let lo = 0, hi = view.t.length;
    while (lo < hi) { const mid = (lo + hi) >> 1; if (view.t[mid] < time) lo = mid + 1; else hi = mid; }
    const i = lo === 0 ? 0 : lo === view.t.length ? lo - 1 : time - view.t[lo - 1] <= view.t[lo] - time ? lo - 1 : lo;
    const value = view.kind === 'raw' ? String(view.v[i]) : `${view.min[i]}…${view.max[i]}`;
    rows.push(`${ch}\t${value}`);
  }
  return rows.join('\n');
}
