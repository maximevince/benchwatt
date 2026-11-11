import { describe, expect, it } from 'vitest';
import type { ChannelDesc, DeviceDesc, SettingDesc } from './backend';
import { channelColors, currentField, outputColor, outputTag, defaultRecordChannels, formatStatus, formatValue, joinLabels, lanesFor, numberField, orderedEntries, outputReadouts, rxChannel, settingError, statusChannels, voltageField, UART_LANE } from './channels';

let id = 0;
const ch = (name: string, quantity: ChannelDesc['quantity'], extra: Partial<ChannelDesc> = {}): ChannelDesc =>
  ({ id: id++, name, label: name.toUpperCase(), unit: quantity === 'Current' ? 'A' : quantity === 'Voltage' ? 'V' : quantity === 'Power' ? 'W' : quantity === 'Temperature' ? '°C' : '', rate_hz: 1000, kind: 'Analog', quantity, group: '', bits: 0, extra: false, set: false, ...extra });
const device = (channels: ChannelDesc[], outputs: DeviceDesc['outputs'] = []): DeviceDesc =>
  ({ id: 'x', kind: 'x', model: 'X', serial: '', firmware: '', hardware: '', channels, outputs, settings: [], gpo: 0, uart: false });
const output = (n: number, group: string, color: string | null = null) => ({ n, label: `Out ${n}`, group, voltage: { min: 0, max: 5 }, voltage_steps: [], current_limit: null, color });

// A meter/supply shaped like a typical bench instrument with one output, extra channels, digital inputs and a UART.
const meter = [
  ch('i', 'Current', { group: 'main' }), ch('v', 'Voltage', { group: 'main' }), ch('i2', 'Current'), ch('v2', 'Voltage'),
  ch('v3', 'Voltage', { extra: true }), ch('p', 'Power', { group: 'main' }), ch('p2', 'Power', { extra: true }), ch('t', 'Temperature'),
  ch('din', 'Other', { kind: 'Digital', bits: 2, label: 'Inputs' }), ch('in', 'Other', { kind: 'Bytes', label: 'UART RX' }), ch('out', 'Other', { kind: 'Bytes', label: 'UART TX', extra: true, set: true }),
];

describe('lane colours', () => {
  it('gives the first of each quantity its colour, the second current or voltage its secondary colour, others aux', () => {
    expect(channelColors(meter)).toEqual({ i: 'current', v: 'voltage', i2: 'teal', v2: 'cyan', v3: 'aux', p: 'power', p2: 'aux', t: 'aux', din: 'digital', in: 'uart', out: 'uart' });
  });
  it('depends only on the channels in view', () => {
    expect(channelColors([meter[2], meter[3]])).toEqual({ i2: 'current', v2: 'voltage' });
    expect(channelColors([ch('a', 'Current'), ch('b', 'Current'), ch('c', 'Current')])).toEqual({ a: 'current', b: 'teal', c: 'aux' });
    expect(channelColors([ch('v', 'Voltage'), ch('v2', 'Voltage'), ch('i', 'Current'), ch('i2', 'Current')])).toEqual({ v: 'voltage', v2: 'cyan', i: 'current', i2: 'teal' });
  });
});

describe('output colours', () => {
  const a = ch('a.i', 'Current', { group: 'a', label: 'Out 1 current' }), b = ch('b.v', 'Voltage', { group: 'b', label: 'Out 2 voltage' }), free = ch('t', 'Temperature');
  it('maps known colour names to a token and anything else to null', () => {
    expect(outputColor('cyan')).toBe('output-cyan');
    expect(outputColor('teal')).toBeNull();
    expect(outputColor(null)).toBeNull();
  });
  it('tags channels with their output when it has a colour or the device has several outputs', () => {
    const marked = [output(1, 'a', 'yellow'), output(2, 'b', 'chartreuse')];
    expect(outputTag(marked, a)).toEqual({ label: 'Out 1', color: 'output-yellow', rest: 'current' });
    expect(outputTag(marked, b)).toEqual({ label: 'Out 2', color: null, rest: 'voltage' });
    expect(outputTag(marked, free)).toBeNull();
    expect(outputTag([output(1, 'a')], a)).toBeNull();
    expect(outputTag([output(1, 'a', 'green')], a)?.color).toBe('output-green');
  });
  it('draws channels of a coloured output in its colour and the rest by quantity', () => {
    expect(channelColors([a, b, ch('v', 'Voltage')], [output(1, 'a', 'magenta'), output(2, 'b')])).toEqual({ 'a.i': 'output-magenta', 'b.v': 'voltage', v: 'cyan' });
    expect(lanesFor([a], [output(1, 'a', 'green'), output(2, 'b')])[0]).toMatchObject({ color: 'output-green', output: { label: 'Out 1', rest: 'current' } });
  });
});

describe('plot lanes', () => {
  it('splits digital channels into bits and merges byte channels into one lane, in description order', () => {
    const lanes = lanesFor([meter[0], meter[8], meter[9], meter[10]]);
    expect(lanes.map(l => [l.key, l.label, l.kind, l.color])).toEqual([
      ['i', 'I', 'analog', 'current'], ['din#1', 'Inputs 1', 'digital', 'digital'], ['din#2', 'Inputs 2', 'digital', 'digital'], [UART_LANE, 'UART RX / TX', 'uart', 'uart'],
    ]);
    expect(lanes[2].bit).toBe(1);
    expect(lanesFor([meter[10]]).map(l => l.label)).toEqual(['UART TX']);
  });
  it('joins labels keeping a shared leading word once', () => {
    expect(joinLabels(['UART RX', 'UART TX'])).toBe('UART RX / TX');
    expect(joinLabels(['Serial in', 'Debug out'])).toBe('Serial in / Debug out');
    expect(joinLabels(['UART'])).toBe('UART');
  });
});

describe('default record channels', () => {
  it('records the non-extra analog channels of the first output', () => {
    expect(defaultRecordChannels(device(meter, [output(1, 'main')]))).toEqual(['i', 'v', 'p']);
    const multi = [ch('a.v', 'Voltage', { group: 'a' }), ch('a.i', 'Current', { group: 'a' }), ch('b.v', 'Voltage', { group: 'b' })];
    expect(defaultRecordChannels(device(multi, [output(1, 'a'), output(2, 'b')]))).toEqual(['a.v', 'a.i']);
  });
  it('records all grouped non-extra analog channels of a meter without outputs', () => {
    const channels = [ch('i', 'Current', { group: 'm' }), ch('v', 'Voltage', { group: 'm', extra: true }), ch('t', 'Temperature'), ch('d', 'Other', { kind: 'Digital', bits: 8, group: 'm' })];
    expect(defaultRecordChannels(device(channels))).toEqual(['i']);
  });
});

describe('sidebar selections', () => {
  const desc = device(meter, [output(1, 'main')]);
  it('reads an output as voltage, current, power', () => {
    expect(outputReadouts(desc, desc.outputs[0]).map(c => c.name)).toEqual(['v', 'i', 'p']);
  });
  it('lists only ungrouped analog channels that have a reading as status', () => {
    expect(statusChannels(desc, { i: 1, v: 2, t: 23, v2: 5 }).map(c => c.name)).toEqual(['v2', 't']);
  });
  it('finds the received byte channel', () => {
    expect(rxChannel(meter)?.name).toBe('in');
  });
  it('orders per-channel records by the description', () => {
    expect(orderedEntries({ p: 3, zz: 9, i: 1, v: 2 }, meter).map(([name]) => name)).toEqual(['i', 'v', 'p', 'zz']);
  });
});

describe('formatting', () => {
  it('auto-ranges current, voltage and power and prints others with their unit', () => {
    expect(formatValue(0.0184, meter[0])).toBe('18.40 mA');
    expect(formatValue(23.456, meter[7])).toBe('23.46 °C');
    expect(formatStatus(23.04, meter[7])).toBe('23.0 °C');
    expect(formatStatus(5.05, meter[3])).toBe('5.05 V');
    expect(formatStatus(null, meter[3])).toBe('—');
  });
  it('picks setpoint units from the range', () => {
    expect(voltageField({ min: 0, max: 3.76 })).toMatchObject({ precision: 3, hint: 'max 3.76 V' });
    expect(voltageField({ min: 0.8, max: 5 }).hint).toBe('0.8–5 V');
    expect(voltageField({ min: 0, max: 32 }).precision).toBe(2);
    expect(currentField({ min: 0.001, max: 3 })).toMatchObject({ scale: 1000, unit: 'mA', min: 1, max: 3000, precision: 0, hint: '1–3000 mA' });
    expect(currentField({ min: 0, max: 20 })).toMatchObject({ scale: 1, unit: 'A', precision: 3 });
  });
});

describe('setting validation', () => {
  const setting = (extra: Partial<SettingDesc>): SettingDesc => ({ key: 'k', label: 'Mode', type: 'bool', output: null, readonly: false, ...extra });
  it('checks the value type', () => {
    expect(settingError(setting({}), true)).toBeNull();
    expect(settingError(setting({}), 1)).toMatch(/on or off/);
  });
  it('accepts only listed choices', () => {
    const choice = setting({ type: 'choice', options: ['a', 'b'] });
    expect(settingError(choice, 'b')).toBeNull();
    expect(settingError(choice, 'c')).toMatch(/one of a, b/);
  });
  it('keeps numbers within min..max', () => {
    const number = setting({ type: 'number', label: 'Level', min: 1.2, max: 5, unit: 'V' });
    expect(settingError(number, 3)).toBeNull();
    expect(settingError(number, 5.1)).toBe('Level must be between 1.2 and 5 V');
    expect(settingError(number, NaN)).toMatch(/number/);
    expect(numberField(number)).toMatchObject({ precision: 2, hint: '1.2–5 V', unit: 'V' });
  });
  it('rejects readonly settings', () => {
    expect(settingError(setting({ readonly: true }), true)).toMatch(/read-only/);
  });
});
