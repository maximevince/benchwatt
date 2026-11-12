import type { SerialInfo } from './backend';
import { defaultSerialSettings } from './serial';
import type { Backend, Command, Events, Devices, DeviceDesc, DeviceState, ChannelDesc, Monitor, RecordingInfo, View, AllStats, UartChunk, Edge, Stats, OutputDesc, SettingDesc } from './backend';
// A device that cannot report a setpoint leaves it null; the simulation then assumes 3.7 V and no limit.
const setpoints = (sim: Sim, i: number) => { const o = sim.state.outputs[i]; return { enabled: o.enabled, voltage: o.voltage ?? 3.7, current_limit: o.current_limit ?? Infinity }; };
import { settingError } from './channels';

// Browser stand-in for the Tauri backend: simulates concrete devices through the same contract.
// ?devices=arc,gpp,ppk2 picks what is plugged in (default: one Arc); ?disconnected simulates nothing.
const params = new URLSearchParams(window.location.search);
const kinds = params.has('disconnected') ? [] : (params.get('devices') ?? 'arc').split(',').map(x => x.trim()).filter(Boolean);

const listeners = new Map<keyof Events, Set<(value: never) => void>>();
function emit<K extends keyof Events>(name: K, value: Events[K]) {
  for (const handler of listeners.get(name) ?? []) handler(value as never);
}
function fail(message: string): never { throw new Error(message); }
function finite(value: unknown): value is number { return typeof value === 'number' && Number.isFinite(value); }

type Sim = {
  desc: DeviceDesc; state: DeviceState; phase: number;
  /** Live values for the monitor, called every 100 ms; may switch outputs off. */
  monitor(sim: Sim): Pick<Monitor, 'readings' | 'inputs'>;
  /** Recorded value of an analog channel at time t. */
  sample(sim: Sim, ch: string, t: number): number;
  /** Digital input mask recorded during the 100 ms starting at t, as edges. */
  edges?(sim: Sim, t: number): Edge[];
};
let nextId = 0;
const channel = (name: string, label: string, unit: string, kind: ChannelDesc['kind'], quantity: ChannelDesc['quantity'], rate_hz: number, extra: Partial<ChannelDesc> = {}): ChannelDesc =>
  ({ id: nextId++, name, label, unit, rate_hz, kind, quantity, group: '', bits: 0, extra: false, set: false, ...extra });
const awake = (t: number, period = 1, width = 0.02) => t % period < width;

function arc(id: string): Sim {
  nextId = 0;
  const analog = (name: string, label: string, unit: string, quantity: ChannelDesc['quantity'], extra: Partial<ChannelDesc> = {}) => channel(name, label, unit, 'Analog', quantity, name === 'mc' || name === 'mp' ? 4000 : 1000, extra);
  const channels = [
    analog('mc', 'Current', 'A', 'Current', { group: 'main' }), analog('mv', 'Voltage', 'V', 'Voltage', { group: 'main' }),
    analog('ac', 'ADC current', 'A', 'Current'), analog('av', 'ADC voltage', 'V', 'Voltage'),
    analog('sn', 'Sense −', 'V', 'Voltage', { extra: true }), analog('sp', 'Sense +', 'V', 'Voltage', { extra: true }),
    analog('mp', 'Power', 'W', 'Power', { group: 'main' }), analog('ap', 'ADC power', 'W', 'Power', { extra: true }),
    analog('vb', 'VBUS', 'V', 'Voltage'), analog('vj', 'DC jack', 'V', 'Voltage', { extra: true }), analog('tp', 'Temperature', '°C', 'Temperature'),
    channel('gpi', 'Digital inputs', '', 'Digital', 'Other', 1000, { bits: 2 }),
    channel('rx', 'UART RX', '', 'Bytes', 'Other', 0), channel('tx', 'UART TX', '', 'Bytes', 'Other', 0, { extra: true, set: true }),
  ];
  const settings: SettingDesc[] = [
    { key: 'current_mode', label: 'Current mode', type: 'choice', options: ['cutoff', 'limit'], output: 1, readonly: false },
    { key: 'io_voltage', label: 'Digital voltage', type: 'number', min: 1.2, max: 5, unit: 'V', output: null, readonly: false },
    { key: '5v_pin', label: '5 V pin', type: 'bool', output: null, readonly: false },
    { key: '4wire', label: '4-wire', type: 'bool', output: null, readonly: true },
  ];
  let spike = 0;
  return {
    desc: { id, kind: 'arc', model: 'Arc', serial: '5120A83D42', firmware: '3.1.3', hardware: '1.0', channels, settings, gpo: 2, uart: true,
      outputs: [{ n: 1, label: 'Main output', group: 'main', voltage: { min: 0, max: 3.76 }, voltage_steps: [], current_limit: { min: 0.001, max: 3 }, color: null }] },
    state: { outputs: [{ enabled: false, voltage: 3.3, current_limit: 0.3 }], settings: { current_mode: 'cutoff', io_voltage: 1.9, '5v_pin': false, '4wire': false }, gpo: [false, false], uart_open: false },
    phase: 0,
    monitor(sim) {
      const out = setpoints(sim, 0), limitMode = sim.state.settings.current_mode === 'limit';
      // A short burst lasts one sample at the 10 Hz monitor rate, representing a 5 ms load event.
      if (sim.phase % 97 === 0) spike = 1;
      const voltage = out.enabled ? out.voltage + Math.sin(sim.phase / 27) * 0.0015 : 0;
      let current = (out.enabled ? voltage / 100 + (Math.random() - 0.5) * 0.00035 : 0) * (spike ? 3 : 1);
      spike = 0;
      if (out.enabled && current > out.current_limit) {
        if (!limitMode) {
          sim.state = { ...sim.state, outputs: [{ ...out, enabled: false }] };
          emit('alert', { device: sim.desc.id, id: 6, name: 'main-overcurr', title: 'Overcurrent', detail: 'Output switched off (cutoff mode).', severity: 'error' });
          emit('state', { device: sim.desc.id, state: sim.state });
          current = 0;
        } else current = out.current_limit;
      }
      const mv = sim.state.outputs[0].enabled ? (limitMode && current >= out.current_limit ? Math.min(voltage, out.current_limit * 100) : voltage) : 0;
      return { readings: { mc: current, mv, mp: mv * current, vb: 5.05, vj: 0, tp: 23.0 + Math.sin(sim.phase / 120) * 0.1 }, inputs: sim.state.gpo[0] ? 1 : 0 };
    },
    sample(_sim, ch, t) {
      const mc = awake(t) ? 0.03 : 0.00005;
      return ({ mc, mv: 3.3, mp: mc * 3.3, sp: 3.3, vb: 5.05, tp: 23 } as Record<string, number>)[ch] ?? 0;
    },
    edges: (_sim, t) => Math.round(t * 10) % 10 === 0 ? [{ t, mask: 1 }, { t: t + 0.02, mask: 0 }] : [],
  };
}

function gpp(id: string): Sim {
  nextId = 0;
  const limits = [[32, 3], [32, 3], [5, 1], [15, 1]], loads = [50, 100, 10, 200], colors = ['yellow', 'cyan', 'magenta', 'green'];
  const outputs: OutputDesc[] = limits.map(([volts, amps], i) => ({ n: i + 1, label: `CH${i + 1}`, group: `ch${i + 1}`, voltage: { min: 0, max: volts }, voltage_steps: [], current_limit: { min: 0, max: amps }, color: colors[i] }));
  const channels = outputs.flatMap(out => (['v', 'i', 'p'] as const).map(q => channel(`${out.group}.${q}`, `${out.label} ${q === 'v' ? 'voltage' : q === 'i' ? 'current' : 'power'}`, q === 'v' ? 'V' : q === 'i' ? 'A' : 'W', 'Analog', q === 'v' ? 'Voltage' : q === 'i' ? 'Current' : 'Power', 10, { group: out.group })));
  const settings: SettingDesc[] = outputs.flatMap(out => [
    { key: `ovp${out.n}`, label: 'OVP', type: 'number' as const, min: 0, max: out.voltage.max + 1, unit: 'V', output: out.n, readonly: false },
    { key: `ocp${out.n}`, label: 'OCP', type: 'number' as const, min: 0, max: out.current_limit!.max + 0.1, unit: 'A', output: out.n, readonly: false },
  ]);
  const load = (sim: Sim, i: number) => { const out = setpoints(sim, i); return out.enabled ? Math.min(out.voltage / loads[i], out.current_limit) : 0; };
  const value = (sim: Sim, ch: string, noise: number) => {
    const i = Number(ch.slice(2, 3)) - 1, out = setpoints(sim, i), amps = load(sim, i) * (1 + noise), volts = out.enabled ? (amps >= out.current_limit && out.current_limit > 0 ? amps * loads[i] : out.voltage) : 0;
    return ch.endsWith('.v') ? volts : ch.endsWith('.i') ? amps : volts * amps;
  };
  return {
    desc: { id, kind: 'gpp', model: 'GPP-4323', serial: 'GEW840129', firmware: '1.17', hardware: 'A', channels, outputs, settings, gpo: 0, uart: false },
    state: { outputs: [{ enabled: false, voltage: 5, current_limit: 1 }, { enabled: false, voltage: 3.3, current_limit: 1 }, { enabled: false, voltage: 3.3, current_limit: 0.5 }, { enabled: false, voltage: 12, current_limit: 0.5 }],
      settings: Object.fromEntries(settings.map(s => [s.key, s.max!])), gpo: [], uart_open: false },
    phase: 0,
    monitor(sim) {
      // Over-current protection switches the output off, as the instrument does.
      sim.state.outputs.forEach((out, i) => {
        if (out.enabled && load(sim, i) > (sim.state.settings[`ocp${i + 1}`] as number)) {
          sim.state = { ...sim.state, outputs: sim.state.outputs.map((o, j) => j === i ? { ...o, enabled: false } : o) };
          emit('state', { device: sim.desc.id, state: sim.state });
        }
      });
      return { readings: Object.fromEntries(channels.map(ch => [ch.name, value(sim, ch.name, (Math.random() - 0.5) * 0.002)])), inputs: null };
    },
    sample: (sim, ch, t) => value(sim, ch, Math.sin(t * 3) * 0.002),
  };
}

function ppk2(id: string): Sim {
  nextId = 0;
  const channels = [
    channel('i', 'Current', 'A', 'Analog', 'Current', 100000, { group: 'out' }),
    channel('v', 'Voltage', 'V', 'Analog', 'Voltage', 1000, { group: 'out', set: true }),
    channel('d', 'Digital', '', 'Digital', 'Other', 100000, { bits: 8 }),
  ];
  const current = (sim: Sim, t: number) => sim.state.settings.mode === 'ampere' || sim.state.outputs[0].enabled ? (awake(t, 0.5, 0.01) ? 0.015 : 0.000003) * setpoints(sim, 0).voltage / 3 : 0;
  return {
    desc: { id, kind: 'ppk2', model: 'PPK2', serial: 'C2A1F7E5', firmware: '1.2.1', hardware: '2', channels, gpo: 0, uart: false,
      outputs: [{ n: 1, label: 'Source', group: 'out', voltage: { min: 0.8, max: 5 }, voltage_steps: [], current_limit: null, color: null }],
      settings: [{ key: 'mode', label: 'Mode', type: 'choice', options: ['source', 'ampere'], output: null, readonly: false }] },
    state: { outputs: [{ enabled: false, voltage: null, current_limit: null }], settings: { mode: 'source' }, gpo: [], uart_open: false },
    phase: 0,
    monitor: sim => ({ readings: { i: current(sim, sim.phase / 10 + Math.random() * 0.5), v: setpoints(sim, 0).voltage }, inputs: (sim.phase >> 3) & 0xff }),
    sample: (sim, ch, t) => ch === 'i' ? current(sim, t) : setpoints(sim, 0).voltage,
    edges: (_sim, t) => [{ t, mask: Math.round(t * 10) & 0xff }],
  };
}

const makers: Record<string, (id: string) => Sim> = { arc, gpp, ppk2 };
const names: Record<string, string> = { arc: 'Otii Arc', gpp: 'GW Instek GPP', ppk2: 'Nordic PPK2' };
const sims: Sim[] = [];
let plugged = false, wanted: string | null = null, active: Sim | null = null;
for (const kind of kinds) {
  if (!makers[kind]) continue;
  sims.push(makers[kind](`${kind}@/dev/ttyACM${sims.length}`));
}
const devices = (): Devices => ({
  found: plugged ? sims.map(s => ({ key: s.desc.id, kind: s.desc.kind, name: names[s.desc.kind], port: s.desc.id.split('@')[1] })) : [],
  wanted, active: active ? { desc: active.desc, state: { ...active.state } } : null, error: null,
});
function sim(id: unknown): Sim {
  return active && active.desc.id === id ? active : fail(`device ${String(id)} is not connected`);
}

/** The serial port linked to the device, and the two channels it adds to a recording. */
let serial: SerialInfo = { port: null, open: false, settings: defaultSerialSettings };
const serialChannels: ChannelDesc[] = [
  { id: 0xfff0, name: 'serial.rx', label: 'Serial RX', unit: '', rate_hz: 0, kind: 'Bytes', quantity: 'Other', group: '', bits: 0, extra: false, set: false },
  { id: 0xfff1, name: 'serial.tx', label: 'Serial TX', unit: '', rate_hz: 0, kind: 'Bytes', quantity: 'Other', group: '', bits: 0, extra: false, set: true },
];
/** Adds the serial RX channel to the live recording once a port is open. */
function linkSerial() {
  const r = live();
  if (r && serial.open && !r.info.channels.includes(serialChannels[0])) update(r, { channels: [...r.info.channels, serialChannels[0]], samples: { ...r.info.samples, 'serial.rx': 0 } });
}
/** One recording: what the frontend knows about it and the samples behind it. */
type Rec = {
  info: RecordingInfo; sim: Sim | null; tick: number; pulse: number; uart: UartChunk[]; edges: Edge[];
  values: Record<string, number[]>; pyramid: Record<string, { min: number; max: number }[][]>;
};
const recs = new Map<number, Rec>();
let lastRec = 0;
const rec = (id: number): Rec => recs.get(id) ?? fail('this recording is closed');
const live = () => [...recs.values()].find(r => r.info.active) ?? null;
function newRec(source: Sim, channels: ChannelDesc[], activeNow: boolean): Rec {
  const { kind, model, serial, firmware, hardware } = source.desc;
  const info: RecordingInfo = { id: ++lastRec, active: activeNow, duration_s: 0, samples: Object.fromEntries(channels.map(ch => [ch.name, 0])), gaps: 0, channels, device: activeNow ? source.desc.id : null, source: { kind, model, serial, firmware, hardware } };
  const r: Rec = { info, sim: source, tick: 0, pulse: 0, uart: [], edges: [], values: {}, pyramid: {} };
  recs.set(info.id, r);
  return r;
}
const rate = (r: Rec, ch: string) => r.info.channels.find(c => c.name === ch)?.rate_hz ?? 0;
function add(r: Rec, ch: string, value: number) {
  const raw = r.values[ch] ??= [];
  raw.push(value);
  const levels = r.pyramid[ch] ??= [[]];
  levels[0].push({ min: value, max: value });
  let level = 0;
  while (levels[level].length % 16 === 0) {
    const batch = levels[level].slice(-16);
    levels[level + 1] ??= [];
    levels[level + 1].push({ min: Math.min(...batch.map(x => x.min)), max: Math.max(...batch.map(x => x.max)) });
    level++;
  }
}
/** Appends 100 ms of samples from the recorded device. */
function generate(r: Rec, source: Sim, uartOpen: boolean) {
  const start = r.tick / 10, info = r.info;
  for (const ch of info.channels) {
    if (ch.kind === 'Digital' && source.edges) { r.edges.push(...source.edges(source, start)); info.samples[ch.name] = r.edges.length; }
    else if (ch.kind === 'Bytes' && !ch.set && uartOpen && r.tick % 10 === 0) {
      r.uart.push({ t: start, dir: 'rx', bytes: Array.from(new TextEncoder().encode(`tick ${++r.pulse}\n`)) });
      info.samples[ch.name] = r.uart.filter(x => x.dir === 'rx').reduce((n, x) => n + x.bytes.length, 0);
    } else if (ch.kind === 'Analog') {
      for (let i = 0; i < ch.rate_hz / 10; i++) add(r, ch.name, source.sample(source, ch.name, start + i / ch.rate_hz));
      info.samples[ch.name] = r.values[ch.name].length;
    }
  }
  r.tick++;
}
/** Replaces a recording's info, as the backend does when it changes, and tells the frontend. */
function update(r: Rec, patch: Partial<RecordingInfo>) {
  r.info = { ...r.info, ...patch };
  emit('recording', r.info);
}
function recTick() {
  const r = live();
  if (!r?.sim) return;
  generate(r, r.sim, r.sim.state.uart_open || serial.open);
  update(r, { duration_s: r.tick / 10, samples: { ...r.info.samples } });
}
window.setInterval(recTick, 100);
function mockView(r: Rec, ch: string, t0: number, t1: number, px: number): View {
  const hz = rate(r, ch), raw = r.values[ch] ?? [];
  if (!hz) return { kind: 'raw', t: [], v: [] };
  const lo = Math.max(0, Math.ceil(t0 * hz)), hi = Math.min(raw.length, Math.floor(t1 * hz) + 1);
  if (hi - lo <= 2 * px) {
    const t: number[] = [], v: number[] = [];
    for (let i = lo; i < hi; i++) { t.push(i / hz); v.push(raw[i]); }
    return { kind: 'raw', t, v };
  }
  const levels = r.pyramid[ch];
  let level = 1;
  while (level + 1 < levels.length && (hi - lo) / 16 ** level > 2 * px) level++;
  const size = 16 ** level, bins = levels[level] ?? [];
  const t: number[] = [], min: number[] = [], max: number[] = [];
  for (let i = Math.floor(lo / size); i < Math.min(bins.length, Math.ceil(hi / size)); i++) {
    t.push(i * size / hz); min.push(bins[i].min); max.push(bins[i].max);
  }
  const tailStart = bins.length * size;
  if (tailStart < hi && tailStart < raw.length) {
    const tail = raw.slice(Math.max(tailStart, lo), hi);
    if (tail.length) {
      let low = Infinity, high = -Infinity;
      for (const value of tail) { low = Math.min(low, value); high = Math.max(high, value); }
      t.push(tailStart / hz); min.push(low); max.push(high);
    }
  }
  return { kind: 'envelope', t, min, max };
}
function mockStats(r: Rec, t0: number, t1: number): AllStats {
  const channels: Record<string, Stats> = {};
  for (const [ch, raw] of Object.entries(r.values)) {
    const hz = rate(r, ch), slice = raw.slice(Math.max(0, Math.ceil(t0 * hz)), Math.min(raw.length, Math.floor(t1 * hz) + 1));
    if (!slice.length) continue;
    let sum = 0, square = 0, min = Infinity, max = -Infinity;
    for (const x of slice) { sum += x; square += x*x; min = Math.min(min, x); max = Math.max(max, x); }
    channels[ch] = { count: slice.length, min, max, mean: sum / slice.length, rms: Math.sqrt(square / slice.length), integral: sum / hz };
  }
  const first = (q: string) => r.info.channels.find(c => c.kind === 'Analog' && c.quantity === q && channels[c.name]);
  const current = first('Current'), voltage = first('Voltage'), power = first('Power');
  const charge_mah = current ? channels[current.name].integral / 3.6 : null;
  const joules = power ? channels[power.name].integral : current && voltage ? channels[current.name].integral * channels[voltage.name].mean : null;
  return { channels, charge_mah, energy: joules == null ? null : { joules, mwh: joules / 3.6, average_power: joules / Math.max(t1 - t0, Number.EPSILON) } };
}
function changed(s: Sim): DeviceState {
  emit('state', { device: s.desc.id, state: s.state });
  return { ...s.state };
}
function apply(name: Command, args: Record<string, unknown>): DeviceState {
  const s = sim(args.device), index = s.desc.outputs.findIndex(o => o.n === args.out), output = s.desc.outputs[index];
  const setOutput = (patch: Partial<DeviceState['outputs'][number]>) => { s.state = { ...s.state, outputs: s.state.outputs.map((o, i) => i === index ? { ...o, ...patch } : o) }; };
  switch (name) {
    case 'set_output':
      if (!output) fail('unknown output');
      if (typeof args.on !== 'boolean') fail('invalid output state');
      setOutput({ enabled: args.on }); break;
    case 'set_voltage':
      if (!output) fail('unknown output');
      if (!finite(args.volts) || args.volts < output.voltage.min || args.volts > output.voltage.max) fail(`voltage must be between ${output.voltage.min} and ${output.voltage.max} V`);
      if (output.voltage_steps.length && !output.voltage_steps.includes(args.volts)) fail(`voltage must be one of ${output.voltage_steps.join(', ')} V`);
      setOutput({ voltage: args.volts }); break;
    case 'set_current_limit': {
      if (!output) fail('unknown output');
      const range = output.current_limit ?? fail(`${output.label} has no current limit`);
      if (!finite(args.amps) || args.amps < range.min || args.amps > range.max) fail(`current limit must be between ${range.min} and ${range.max} A`);
      setOutput({ current_limit: args.amps }); break;
    }
    case 'set_setting': {
      const setting = s.desc.settings.find(x => x.key === args.key) ?? fail(`unknown setting ${String(args.key)}`);
      const error = settingError(setting, args.value); if (error) fail(error);
      s.state = { ...s.state, settings: { ...s.state.settings, [setting.key]: args.value as boolean | string | number } }; break;
    }
    case 'set_gpo':
      if (!Number.isInteger(args.n) || (args.n as number) < 1 || (args.n as number) > s.desc.gpo || typeof args.on !== 'boolean') fail('invalid GPO');
      s.state = { ...s.state, gpo: s.state.gpo.map((value, i) => i === (args.n as number) - 1 ? args.on as boolean : value) }; break;
  }
  return changed(s);
}
function tick() {
  if (!active) return;
  active.phase++;
  emit('monitor', { device: active.desc.id, ...active.monitor(active) });
}
window.setInterval(tick, 100);
if (sims.length) window.setTimeout(() => { plugged = true; emit('devices', devices()); }, 1000);
/** The device's channels with these names, in description order. */
const pick = (source: Sim, names: string[]): ChannelDesc[] => source.desc.channels.filter(ch => names.includes(ch.name));
/** What opening a file gives: 5 s of the Arc with its UART. */
function cannedRecording(): RecordingInfo {
  const source = arc('arc'), r = newRec(source, pick(source, ['mc', 'mv', 'mp', 'gpi', 'rx']), false);
  for (let i = 0; i < 50; i++) generate(r, source, true);
  r.info.duration_s = 5; r.sim = null;
  return r.info;
}
function stop() {
  const r = live();
  if (r) update(r, { active: false });
}
export const mockBackend: Backend = {
  getDevices: async () => devices(),
  selectDevice: async key => {
    if (live()) fail('stop the recording before changing the device');
    const next = key === null ? null : sims.find(s => plugged && s.desc.id === key) ?? fail(`device ${key} is not plugged in`);
    wanted = key; active = null;
    if (serial.open) { serial = { ...serial, open: false }; emit('serial', serial); }
    emit('devices', devices());
    // Opening a device takes a moment.
    if (next) window.setTimeout(() => { if (wanted === key) { active = next; emit('devices', devices()); } }, 300);
  },
  command: async (name, args) => apply(name, args),
  listen: async (name, handler) => {
    const set = listeners.get(name) ?? new Set();
    set.add(handler as (value: never) => void);
    listeners.set(name, set);
    return () => { set.delete(handler as (value: never) => void); };
  },
  recordings: async () => [...recs.values()].map(r => r.info),
  startRecording: async (device, channels) => {
    const s = sim(device);
    for (const name of channels) if (!s.desc.channels.some(ch => ch.name === name)) fail(`unknown channel ${name}`);
    stop();
    const rx = s.desc.channels.find(ch => ch.kind === 'Bytes' && !ch.set);
    const r = newRec(s, pick(s, [...channels, ...(s.state.uart_open && rx ? [rx.name] : [])]), true);
    linkSerial();
    return r.info;
  },
  stopRecording: async () => stop(),
  closeRecording: async id => { if (live()?.info.id === id) stop(); recs.delete(id); },
  getView: async (id, ch, t0, t1, px) => mockView(rec(id), ch, t0, t1, px),
  getStats: async (id, t0, t1) => mockStats(rec(id), t0, t1),
  getUart: async (id, t0, t1) => recs.get(id)?.uart.filter(x => x.t >= t0 && x.t <= t1) ?? [],
  getEdges: async (id, t0, t1) => recs.get(id)?.edges.filter(x => x.t >= t0 && x.t <= t1) ?? [],
  saveRecording: async id => { rec(id); },
  loadRecording: async () => cannedRecording(),
  exportRecording: async id => { rec(id); },
  uartOpen: async (device, baud) => {
    const s = sim(device);
    if (!s.desc.uart) fail('device has no UART');
    if (!Number.isInteger(baud) || baud <= 0) fail('invalid baud rate');
    if (serial.open) fail('close the serial port first: a recording has one source of text');
    s.state = { ...s.state, uart_open: true }; changed(s);
    const r = live(), rx = s.desc.channels.find(ch => ch.kind === 'Bytes' && !ch.set);
    if (r?.sim === s && rx && !r.info.channels.includes(rx)) update(r, { channels: pick(s, [...r.info.channels.map(ch => ch.name), rx.name]), samples: { ...r.info.samples, [rx.name]: 0 } });
  },
  uartClose: async device => { const s = sim(device); s.state = { ...s.state, uart_open: false }; changed(s); },
  uartSend: async (device, text) => {
    const s = sim(device);
    if (!s.state.uart_open) fail('UART closed');
    const r = live();
    if (r?.sim !== s) return;
    const tx = s.desc.channels.find(ch => ch.kind === 'Bytes' && ch.set), rx = s.desc.channels.find(ch => ch.kind === 'Bytes' && !ch.set);
    const t = r.info.duration_s, bytes = Array.from(new TextEncoder().encode(text));
    r.uart.push({ t, dir: 'tx', bytes });
    if (tx) update(r, { channels: pick(s, [...r.info.channels.map(ch => ch.name), tx.name]), samples: { ...r.info.samples, [tx.name]: (r.info.samples[tx.name] ?? 0) + bytes.length } });
    window.setTimeout(() => {
      if (!r.info.active || !rx) return;
      // The device splits the echo over 1 ms packets; the recorder joins them back into one chunk.
      r.uart.push({ t: t + .001, dir: 'rx', bytes });
      update(r, { samples: { ...r.info.samples, [rx.name]: (r.info.samples[rx.name] ?? 0) + bytes.length } });
    }, 1);
  },
  serialPorts: async () => [{ path: '/dev/ttyUSB1', name: 'FT232R USB UART' }, { path: '/dev/ttyACM3', name: 'J-Link CDC UART' }, { path: '/dev/ttyS0', name: '' }],
  serialInfo: async () => serial,
  serialOpen: async (device, port, settings) => {
    if (sim(device).state.uart_open) fail("close the device's UART first: a recording has one source of text");
    if (port === '/dev/ttyS0') fail(`${port}: Permission denied`);
    serial = { port, open: true, settings };
    emit('serial', serial);
    linkSerial();
  },
  serialClose: async () => { serial = { ...serial, open: false }; emit('serial', serial); },
  serialSend: async text => {
    if (!serial.open) fail('no serial port is open');
    const r = live();
    if (!r) return;
    const bytes = Array.from(new TextEncoder().encode(text));
    r.uart.push({ t: r.info.duration_s, dir: 'tx', bytes });
    update(r, { channels: r.info.channels.includes(serialChannels[1]) ? r.info.channels : [...r.info.channels, serialChannels[1]], samples: { ...r.info.samples, 'serial.tx': (r.info.samples['serial.tx'] ?? 0) + bytes.length } });
  },
  serialLines: async ({ dtr, rts }) => {
    if (!serial.open) fail('no serial port is open');
    serial = { ...serial, settings: { ...serial.settings, ...(dtr === undefined ? {} : { dtr }), ...(rts === undefined ? {} : { rts }) } };
    emit('serial', serial);
  },
};
