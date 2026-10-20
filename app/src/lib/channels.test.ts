import { describe, expect, it } from 'vitest';
import { channel, channels } from './channels';

describe('channel labels', () => {
  it('provides the required human names and units', () => {
    expect(channel('mc')).toMatchObject({ label: 'Current', unit: 'A', unitKind: 'current' });
    expect(channel('ap')).toMatchObject({ label: 'ADC power', unit: 'W', unitKind: 'power' });
    expect(channel('gpi2')).toMatchObject({ label: 'Digital in 2', kind: 'digital' });
    expect(channel('tx').label).toBe('UART TX');
    expect(Object.keys(channels)).toHaveLength(14);
  });
  it('falls back to the protocol id for unknown channels', () => {
    expect(channel('future').label).toBe('future');
    expect(channel('future').short).toBe('future');
  });
});
