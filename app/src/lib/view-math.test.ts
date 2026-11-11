import { describe, expect, it } from 'vitest';
import { niceTicks, panFrom, zoomAt } from './view-math';

describe('nice axis ticks', () => {
  it('uses round 1-2-5 steps', () => {
    expect(niceTicks(0, 30, 5)).toEqual([0, 10, 20, 30]);
    expect(niceTicks(-0.8, 0.8, 5)).toEqual([-1, -0.5, 0, 0.5, 1]);
  });
  it('chooses the required decimal precision', () => {
    expect(niceTicks(0.1, 0.3, 4)).toEqual([0.1, 0.2, 0.3]);
  });
});

describe('recording view interactions', () => {
  it('holds the time beneath the pointer when wheel zooming', () => {
    const before: [number, number] = [10, 30];
    const anchor = 15;
    const after = zoomAt(before, anchor, 0.5, 100, 0.01);
    expect((anchor - after[0]) / (after[1] - after[0])).toBeCloseTo(0.25);
    expect(after).toEqual([12.5, 22.5]);
  });
  it('clamps zoom and pans without changing span', () => {
    expect(zoomAt([10, 30], 15, 100, 100, 0.01)).toEqual([0, 100]);
    expect(zoomAt([10, 30], 15, 0.00001, 100, 0.01)[1] - zoomAt([10, 30], 15, 0.00001, 100, 0.01)[0]).toBeCloseTo(0.01);
    expect(panFrom([10, 30], 50, 100, 100, 0.01)).toEqual([0, 20]);
    expect(panFrom([10, 30], -50, 100, 100, 0.01)).toEqual([20, 40]);
  });
});
