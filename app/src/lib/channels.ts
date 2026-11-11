import type { ChannelDesc, DeviceDesc, OutputDesc, Quantity, Range, SettingDesc } from './backend';
import { formatReading } from './format';

/** Readout kinds with auto-ranged units and their own lane colour. */
export type ReadingKind = 'current' | 'voltage' | 'power';
export function readingKind(quantity: Quantity): ReadingKind | null {
  return quantity === 'Current' ? 'current' : quantity === 'Voltage' ? 'voltage' : quantity === 'Power' ? 'power' : null;
}

const OUTPUT_COLORS = ['yellow', 'cyan', 'magenta', 'green', 'blue', 'red', 'orange'];
/** Colour token (a CSS variable name) of an output's front-panel marking, or null for none or a colour we have no shade for. */
export function outputColor(name: string | null | undefined): string | null {
  return name && OUTPUT_COLORS.includes(name) ? 'output-' + name : null;
}

/** The output a channel belongs to, as a chip: shown when the output has a colour or the device has several outputs.
 * `rest` is the channel label without the output's name ("CH2 current" → "current"). */
export type OutputTag = { label: string; color: string | null; rest: string };
export function outputTag(outputs: OutputDesc[], ch: ChannelDesc): OutputTag | null {
  const out = outputs.find(o => o.group === ch.group), color = outputColor(out?.color);
  if (!out || (outputs.length < 2 && !color)) return null;
  const prefix = out.label + ' ';
  return { label: out.label, color, rest: ch.label.startsWith(prefix) ? ch.label.slice(prefix.length) : ch.label };
}

/** Lane colour token per channel name. A channel of a coloured output takes the output's colour, like on the device.
 * Otherwise the first current/voltage/power of a list gets its quantity's colour,
 * the second current or voltage its secondary colour so two of them stay distinguishable, any others the aux colour. */
export function channelColors(channels: ChannelDesc[], outputs: OutputDesc[] = []): Record<string, string> {
  const secondary: Record<string, string | undefined> = { current: 'teal', voltage: 'cyan' }, seen = new Set<string>(), colors: Record<string, string> = {};
  for (const ch of channels) {
    const kind = readingKind(ch.quantity), marked = outputColor(outputs.find(o => o.group === ch.group)?.color);
    if (ch.kind === 'Digital') colors[ch.name] = 'digital';
    else if (ch.kind === 'Bytes') colors[ch.name] = 'uart';
    else if (marked) colors[ch.name] = marked;
    else if (!kind) colors[ch.name] = 'aux';
    else if (!seen.has(kind)) { seen.add(kind); colors[ch.name] = kind; }
    else { colors[ch.name] = secondary[kind] ?? 'aux'; secondary[kind] = undefined; }
  }
  return colors;
}

/** A value with its unit, auto-ranged for current, voltage and power. */
export function formatValue(value: number, ch: ChannelDesc): string {
  if (!Number.isFinite(value)) return '—';
  const kind = readingKind(ch.quantity);
  if (kind) { const r = formatReading(value, kind); return r.number.trim() + ' ' + r.unit; }
  return (value.toFixed(2) + ' ' + ch.unit).trim();
}
/** A compact live reading for the status list. */
export function formatStatus(value: number | null | undefined, ch: ChannelDesc): string {
  if (value == null || !Number.isFinite(value)) return '—';
  if (ch.quantity === 'Current' || ch.quantity === 'Power') return formatValue(value, ch);
  return (value.toFixed(ch.quantity === 'Temperature' ? 1 : 2) + ' ' + ch.unit).trim();
}

export type Lane = { key: string; label: string; unit: string; color: string; kind: 'analog' | 'digital' | 'uart'; channel: ChannelDesc; bit: number; output: OutputTag | null };
export const UART_LANE = '#uart';

/** Labels joined with a shared leading word kept once: "UART RX", "UART TX" → "UART RX / TX". */
export function joinLabels(labels: string[]): string {
  if (labels.length < 2) return labels[0] ?? '';
  const words = labels.map(label => label.split(' '));
  let common = 0;
  while (words.every(w => w.length > common + 1 && w[common] === words[0][common])) common++;
  return common ? words[0].slice(0, common).join(' ') + ' ' + words.map(w => w.slice(common).join(' ')).join(' / ') : labels.join(' / ');
}

/** Plot lanes in description order: one per analog channel, one per digital bit, one for all byte channels. */
export function lanesFor(channels: ChannelDesc[], outputs: OutputDesc[] = []): Lane[] {
  const colors = channelColors(channels, outputs), bytes = channels.filter(ch => ch.kind === 'Bytes'), lanes: Lane[] = [];
  for (const ch of channels) {
    if (ch.kind === 'Analog') lanes.push({ key: ch.name, label: ch.label, unit: ch.unit, color: colors[ch.name], kind: 'analog', channel: ch, bit: 0, output: outputTag(outputs, ch) });
    else if (ch.kind === 'Digital') for (let i = 1; i <= ch.bits; i++) lanes.push({ key: `${ch.name}#${i}`, label: `${ch.label} ${i}`, unit: '', color: 'digital', kind: 'digital', channel: ch, bit: i - 1, output: null });
    else if (ch === bytes[0]) lanes.push({ key: UART_LANE, label: joinLabels(bytes.map(b => b.label)), unit: '', color: 'uart', kind: 'uart', channel: ch, bit: 0, output: null });
  }
  return lanes;
}

/** Entries of a per-channel record in the order of the description list (unknown names last). */
export function orderedEntries<T>(record: Record<string, T>, channels: ChannelDesc[]): [string, T][] {
  const order = channels.map(ch => ch.name), rank = (name: string) => { const i = order.indexOf(name); return i < 0 ? order.length : i; };
  return Object.entries(record).sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b));
}

/** Channels recorded unless the user picks others: the non-extra analog channels of the first output's group,
 * or for a device without outputs all its grouped non-extra analog channels. */
export function defaultRecordChannels(desc: DeviceDesc): string[] {
  const group = desc.outputs[0]?.group;
  return desc.channels.filter(ch => ch.kind === 'Analog' && !ch.extra && (group === undefined ? ch.group !== '' : ch.group === group)).map(ch => ch.name);
}

/** Voltage, current and power readouts of an output: the first channel of each quantity in its group. */
export function outputReadouts(desc: DeviceDesc, output: OutputDesc): ChannelDesc[] {
  const group = desc.channels.filter(ch => ch.kind === 'Analog' && ch.group === output.group);
  return (['Voltage', 'Current', 'Power'] as const).flatMap(q => { const ch = group.find(c => c.quantity === q && !c.extra) ?? group.find(c => c.quantity === q); return ch ? [ch] : []; });
}

/** Analog channels with a live reading that belong to no output. */
export function statusChannels(desc: DeviceDesc, readings: Record<string, number>): ChannelDesc[] {
  const groups = new Set(desc.outputs.map(out => out.group));
  return desc.channels.filter(ch => ch.kind === 'Analog' && !groups.has(ch.group) && ch.name in readings);
}

/** The byte channel received from the device (always recorded while its UART is open). */
export function rxChannel(channels: ChannelDesc[]): ChannelDesc | undefined {
  return channels.find(ch => ch.kind === 'Bytes' && !ch.set);
}

const trim = (value: number) => String(Number(value.toFixed(3)));
/** How a voltage setpoint is edited: volts, 3 decimals below 10 V, else 2. */
export function voltageField(range: Range) {
  const precision = range.max >= 10 ? 2 : 3;
  return { precision, step: 10 ** -precision, hint: range.min === 0 ? `max ${range.max.toFixed(2)} V` : `${trim(range.min)}–${trim(range.max)} V` };
}
/** How a current limit is edited: whole milliamps up to 10 A, else amps with 3 decimals. */
export function currentField(range: Range) {
  const scale = range.max < 10 ? 1000 : 1, unit = scale === 1000 ? 'mA' : 'A', precision = scale === 1000 ? 0 : 3;
  return { scale, unit, precision, step: 10 ** -precision, min: range.min * scale, max: range.max * scale, hint: `${trim(range.min * scale)}–${trim(range.max * scale)} ${unit}` };
}
/** How a numeric setting is edited: 2 decimals for spans under 100, else 1. */
export function numberField(setting: SettingDesc) {
  const min = setting.min ?? -Infinity, max = setting.max ?? Infinity;
  const precision = max - min >= 100 ? 1 : 2;
  const unit = setting.unit ?? '';
  const hint = Number.isFinite(min) && Number.isFinite(max) ? `${trim(min)}–${trim(max)}${unit ? ' ' + unit : ''}` : '';
  return { min, max, precision, step: 10 ** (1 - precision), unit, hint };
}

/** Why a value can't be applied to a setting, or null when it can (mirrors the backend's checks). */
export function settingError(setting: SettingDesc, value: unknown): string | null {
  if (setting.readonly) return `${setting.label} is read-only`;
  if (setting.type === 'bool') return typeof value === 'boolean' ? null : `${setting.label} must be on or off`;
  if (setting.type === 'choice') return typeof value === 'string' && (setting.options ?? []).includes(value) ? null : `${setting.label} must be one of ${(setting.options ?? []).join(', ')}`;
  if (typeof value !== 'number' || !Number.isFinite(value)) return `${setting.label} must be a number`;
  if ((setting.min != null && value < setting.min) || (setting.max != null && value > setting.max)) return `${setting.label} must be between ${setting.min ?? '−∞'} and ${setting.max ?? '∞'}${setting.unit ? ' ' + setting.unit : ''}`;
  return null;
}

/** A choice option as shown in a select. */
export const optionLabel = (option: string) => option.charAt(0).toUpperCase() + option.slice(1);
