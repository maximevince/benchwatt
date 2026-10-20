import { describe, expect, it } from 'vitest';
import { formatReading } from './format';
describe('auto-ranged readouts', () => {
  it('selects current units at boundaries', () => {
    expect(formatReading(0.0000005, 'current').unit).toBe('nA');
    expect(formatReading(0.000002, 'current').unit).toBe('µA');
    expect(formatReading(0.01842, 'current').unit).toBe('mA');
    expect(formatReading(1.2, 'current').unit).toBe('A');
  });
  it('keeps number fields the same width', () => {
    const values = [null, 0, 1e-9, 0.01842, 3.2].map(v => formatReading(v, 'current').number);
    expect(new Set(values.map(v => v.length))).toEqual(new Set([8]));
  });
  it('handles power, negative readings, and unavailable values', () => {
    expect(formatReading(0.0607, 'power')).toEqual({ number: '   60.70', unit: 'mW' });
    expect(formatReading(-0.001, 'current').unit).toBe('mA');
    expect(formatReading(0, 'voltage').unit).toBe('V');
    expect(formatReading(0, 'current').unit).toBe('mA');
    expect(formatReading(NaN, 'voltage').number.trim()).toBe('—');
  });
});
