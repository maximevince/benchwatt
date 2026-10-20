import { expect, test } from 'vitest';
import { valuesAtTimeTsv } from './point-values';
import type { View } from './view';

test('copies the nearest loaded value for each analog lane', () => {
  const views = new Map<string, View>([
    ['mc', { kind: 'raw', t: [0, 1, 2], v: [1, 2, 3] }],
    ['mv', { kind: 'envelope', t: [0, 1], min: [4, 5], max: [6, 7] }],
  ]);
  expect(valuesAtTimeTsv(0.8, ['mc', 'mv', 'mp'], views)).toBe('channel\tvalue\ntime\t0.800000 s\nmc\t2\nmv\t5…7\nmp\t—');
});
