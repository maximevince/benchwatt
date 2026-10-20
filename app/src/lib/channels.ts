export type UnitKind = 'current' | 'voltage' | 'power' | 'temperature' | 'digital' | 'uart';
export type ChannelKind = 'analog' | 'digital' | 'uart';
export type Channel = { label: string; short: string; unit: string; unitKind: UnitKind; color: string; kind: ChannelKind };

export const channels: Record<string, Channel> = {
  mc: { label: 'Current', short: 'Current', unit: 'A', unitKind: 'current', color: 'current', kind: 'analog' },
  mv: { label: 'Voltage', short: 'Voltage', unit: 'V', unitKind: 'voltage', color: 'voltage', kind: 'analog' },
  mp: { label: 'Power', short: 'Power', unit: 'W', unitKind: 'power', color: 'power', kind: 'analog' },
  ac: { label: 'ADC current', short: 'ADC current', unit: 'A', unitKind: 'current', color: 'teal', kind: 'analog' },
  av: { label: 'ADC voltage', short: 'ADC voltage', unit: 'V', unitKind: 'voltage', color: 'cyan', kind: 'analog' },
  ap: { label: 'ADC power', short: 'ADC power', unit: 'W', unitKind: 'power', color: 'power', kind: 'analog' },
  sn: { label: 'Sense −', short: 'Sense −', unit: 'V', unitKind: 'voltage', color: 'aux', kind: 'analog' },
  sp: { label: 'Sense +', short: 'Sense +', unit: 'V', unitKind: 'voltage', color: 'aux', kind: 'analog' },
  vb: { label: 'VBUS', short: 'VBUS', unit: 'V', unitKind: 'voltage', color: 'aux', kind: 'analog' },
  vj: { label: 'DC jack', short: 'DC jack', unit: 'V', unitKind: 'voltage', color: 'aux', kind: 'analog' },
  tp: { label: 'Temperature', short: 'Temp', unit: '°C', unitKind: 'temperature', color: 'aux', kind: 'analog' },
  gpi: { label: 'Digital inputs', short: 'Digital in', unit: '', unitKind: 'digital', color: 'digital', kind: 'digital' },
  rx: { label: 'UART RX', short: 'RX', unit: '', unitKind: 'uart', color: 'uart', kind: 'uart' },
  tx: { label: 'UART TX', short: 'TX', unit: '', unitKind: 'uart', color: 'uart', kind: 'uart' },
};

/** GPI inputs wired on the Arc's expansion port (the gpi mask has 6 bits). */
export const GPI_INPUTS = [1, 2];

export function channel(id: string): Channel {
  if (/^gpi[1-6]$/.test(id)) return { ...channels.gpi, label: `Digital in ${id.slice(3)}`, short: `Digital in ${id.slice(3)}` };
  return channels[id] ?? { label: id, short: id, unit: '', unitKind: 'voltage', color: 'aux', kind: 'analog' };
}

const order = Object.keys(channels);
/** Entries of a per-channel record in the fixed order of `channels` (unknown ids last). */
export function orderedEntries<T>(record: Record<string, T>): [string, T][] {
  const rank = (id: string) => { const i = order.indexOf(id); return i < 0 ? order.length : i; };
  return Object.entries(record).sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b));
}
