import type { Backend, Command, Events, Settings, Monitor, Connection, RecordingInfo, View, AllStats, UartChunk, Edge, Stats } from './backend';

const disconnected = new URLSearchParams(window.location.search).has('disconnected');
let connection: Connection = { connected: false };
let settings: Settings = { main: false, mainvolt: 3.3, maxcur: 0.3, mainsrccurrmode: 0, mainvolt_max: 3.76, expvolt: 3.3, gpo: [false, false, false], fourwire: false };
const listeners = new Map<keyof Events, Set<(value: never) => void>>();
function emit<K extends keyof Events>(name: K, value: Events[K]) {
  for (const handler of listeners.get(name) ?? []) handler(value as never);
}
let phase = 0;
let spike = 0;
const rates: Record<string, number> = { mc: 4000, mv: 1000, mp: 4000, ac: 1000, av: 1000, vb: 1000, tp: 1000 };
let recording: RecordingInfo | null = null;
let uartOpened = false;
let pulse = 0;
let recordingTick = 0;
let uartLog: UartChunk[] = [];
let edges: Edge[] = [];
const values: Record<string, number[]> = {};
const pyramid: Record<string, { min: number; max: number }[][]> = {};
function add(ch: string, value: number) {
  const raw = values[ch] ??= [];
  raw.push(value);
  const levels = pyramid[ch] ??= [[]];
  levels[0].push({ min: value, max: value });
  let level = 0;
  while (levels[level].length % 16 === 0) {
    const batch = levels[level].slice(-16);
    levels[level + 1] ??= [];
    levels[level + 1].push({ min: Math.min(...batch.map(x => x.min)), max: Math.max(...batch.map(x => x.max)) });
    level++;
  }
}
function recTick() {
  if (!recording?.active) return;
  const start = recordingTick / 10;
  if (recordingTick % 10 === 0) {
    if (recording.channels.includes('gpi')) {
      edges.push({ t: start, mask: 1 }, { t: start + 0.02, mask: 0 });
      recording.samples.gpi = edges.length;
    }
    if (recording.channels.includes('rx')) {
      uartLog.push({ t: start, bytes: Array.from(new TextEncoder().encode(`tick ${++pulse}\n`)) });
      recording.samples.rx = uartLog.reduce((n, x) => n + x.bytes.length, 0);
    }
  }
  for (const ch of recording.channels) {
    if (!(ch in rates)) continue;
    const rate = rates[ch];
    for (let i = 0; i < rate / 10; i++) {
      const t = start + i / rate;
      const awake = t % 1 < 0.02;
      const mc = awake ? 0.03 : 0.00005;
      const value = ({ mc, mv: 3.3, mp: mc * 3.3, ac: 0, av: 0, vb: 5.05, tp: 23 } as Record<string, number>)[ch];
      add(ch, value);
    }
    recording.samples[ch] = values[ch].length;
  }
  recordingTick++;
  recording = { ...recording, duration_s: recordingTick / 10, samples: { ...recording.samples } };
  emit('recording', recording);
}
window.setInterval(recTick, 100);
function mockView(ch: string, t0: number, t1: number, px: number): View {
  const rate = rates[ch], raw = values[ch] ?? [];
  if (!rate) return { kind: 'raw', t: [], v: [] };
  const lo = Math.max(0, Math.ceil(t0 * rate)), hi = Math.min(raw.length, Math.floor(t1 * rate) + 1);
  if (hi - lo <= 2 * px) {
    const t: number[] = [], v: number[] = [];
    for (let i = lo; i < hi; i++) { t.push(i / rate); v.push(raw[i]); }
    return { kind: 'raw', t, v };
  }
  const levels = pyramid[ch];
  let level = 1;
  while (level + 1 < levels.length && (hi - lo) / 16 ** level > 2 * px) level++;
  const size = 16 ** level, bins = levels[level] ?? [];
  const t: number[] = [], min: number[] = [], max: number[] = [];
  for (let i = Math.floor(lo / size); i < Math.min(bins.length, Math.ceil(hi / size)); i++) {
    t.push(i * size / rate); min.push(bins[i].min); max.push(bins[i].max);
  }
  const tailStart = bins.length * size;
  if (tailStart < hi && tailStart < raw.length) {
    const tail = raw.slice(Math.max(tailStart, lo), hi);
    if (tail.length) {
      let low = Infinity, high = -Infinity;
      for (const value of tail) { low = Math.min(low, value); high = Math.max(high, value); }
      t.push(tailStart / rate); min.push(low); max.push(high);
    }
  }
  return { kind: 'envelope', t, min, max };
}
function mockStats(t0: number, t1: number): AllStats {
  const channels: Record<string, Stats> = {};
  for (const [ch, raw] of Object.entries(values)) {
    const rate = rates[ch], slice = raw.slice(Math.max(0, Math.ceil(t0 * rate)), Math.min(raw.length, Math.floor(t1 * rate) + 1));
    if (!slice.length) continue;
    let sum = 0, square = 0, min = Infinity, max = -Infinity;
    for (const x of slice) { sum += x; square += x*x; min = Math.min(min, x); max = Math.max(max, x); }
    channels[ch] = { count: slice.length, min, max, mean: sum / slice.length, rms: Math.sqrt(square / slice.length), integral: sum / rate };
  }
  const charge_mah = channels.mc ? channels.mc.integral / 3.6 : null;
  const joules = channels.mp?.integral ?? (channels.mc ? channels.mc.integral * 3.3 : null);
  return { channels, charge_mah, energy: joules == null ? null : { joules, mwh: joules / 3.6, average_power: joules / Math.max(t1 - t0, Number.EPSILON) } };
}
function tick() {
  if (!connection.connected) return;
  phase++;
  // A short burst lasts one sample at the 10 Hz monitor rate, representing a 5 ms load event.
  if (phase % 97 === 0) spike = 1;
  const voltage = settings.main ? (settings.mainvolt ?? 0) + Math.sin(phase / 27) * 0.0015 : 0;
  const baseCurrent = settings.main ? voltage / 100 + (Math.random() - 0.5) * 0.00035 : 0;
  let current = baseCurrent * (spike ? 3 : 1);
  spike = 0;
  const limit = settings.maxcur ?? 3;
  if (settings.main && current > limit) {
    if (settings.mainsrccurrmode === 0) {
      settings = { ...settings, main: false };
      emit('alert', { id: 6, name: 'main-overcurr' });
      emit('settings', { ...settings });
      current = 0;
    } else current = limit;
  }
  const mv = settings.main ? (settings.mainsrccurrmode === 1 && current >= limit ? Math.min(voltage, limit * 100) : voltage) : 0;
  const monitor: Monitor = { mv, mc: current, mp: mv * current, vb: 5.05, vj: 0, tp: 23.0 + Math.sin(phase / 120) * 0.1, gpi: settings.gpo[0] ? 1 : 0, fourwire: settings.fourwire };
  emit('monitor', monitor);
}
if (!disconnected) window.setTimeout(() => {
  connection = { connected: true, info: { name: 'Otii Arc', fw: '3.1.3', hw: '1.0', uid: '5120A83D42' } };
  emit('connection', connection);
  emit('settings', { ...settings });
  tick();
  window.setInterval(tick, 100);
}, 1000);
function finite(value: unknown): value is number { return typeof value === 'number' && Number.isFinite(value); }
function fail(message: string): never { throw new Error(message); }
function apply(name: Command, args: Record<string, unknown>) {
  if (!connection.connected) fail('device disconnected');
  switch (name) {
    case 'set_output':
      if (typeof args.on !== 'boolean') fail('invalid output state');
      settings = { ...settings, main: args.on as boolean }; break;
    case 'set_voltage':
      if (!finite(args.volts) || args.volts < 0 || args.volts > (settings.mainvolt_max ?? Infinity)) fail(`voltage must be between 0 and ${settings.mainvolt_max} V`);
      settings = { ...settings, mainvolt: args.volts as number }; break;
    case 'set_current_limit':
      if (!finite(args.amps) || args.amps < 0.001 || args.amps > 3) fail('current limit must be between 0.001 and 3.0 A');
      settings = { ...settings, maxcur: args.amps as number }; break;
    case 'set_current_mode':
      if (args.mode !== 'cutoff' && args.mode !== 'limit') fail('mode must be cutoff or limit');
      settings = { ...settings, mainsrccurrmode: args.mode === 'limit' ? 1 : 0 }; break;
    case 'set_gpo':
      if (!Number.isInteger(args.n) || (args.n as number) < 1 || (args.n as number) > 3 || typeof args.on !== 'boolean') fail('invalid GPO');
      settings = { ...settings, gpo: settings.gpo.map((value, index) => index === (args.n as number) - 1 ? args.on as boolean : value) }; break;
  }
  emit('settings', { ...settings });
  return { ...settings };
}
export const mockBackend: Backend = {
  getState: async () => ({ connection, settings: connection.connected ? { ...settings } : null }),
  command: async (name, args) => apply(name, args),
  listen: async (name, handler) => {
    const set = listeners.get(name) ?? new Set();
    set.add(handler as (value: never) => void);
    listeners.set(name, set);
    return () => { set.delete(handler as (value: never) => void); };
  },
  startRecording: async channels => {
    if (!connection.connected) fail('device disconnected');
    for (const key of Object.keys(values)) delete values[key];
    for (const key of Object.keys(pyramid)) delete pyramid[key];
    edges = []; uartLog = []; pulse = 0; recordingTick = 0;
    const selected = [...new Set([...channels, ...(uartOpened ? ['rx'] : [])])];
    recording = { active: true, duration_s: 0, samples: Object.fromEntries(selected.map(ch => [ch, 0])), gaps: 0, channels: selected };
    emit('recording', recording);
    return recording;
  },
  stopRecording: async () => { if (recording) { recording = { ...recording, active: false }; emit('recording', recording); } return recording; },
  recordingInfo: async () => recording,
  getView: async (ch, t0, t1, px) => mockView(ch, t0, t1, px),
  getStats: async (t0, t1) => mockStats(t0, t1),
  getUart: async (t0, t1) => uartLog.filter(x => x.t >= t0 && x.t <= t1),
  getEdges: async (t0, t1) => edges.filter(x => x.t >= t0 && x.t <= t1),
  uartOpen: async baud => { if (!Number.isInteger(baud) || baud <= 0) fail('invalid baud rate'); uartOpened = true; if (recording?.active && !recording.channels.includes('rx')) recording = { ...recording, channels: [...recording.channels, 'rx'], samples: { ...recording.samples, rx: 0 } }; },
  uartClose: async () => { uartOpened = false; },
  uartSend: async text => { if (!uartOpened) fail('UART closed'); uartLog.push({ t: recording?.duration_s ?? 0, bytes: Array.from(new TextEncoder().encode(text)) }); },
};
