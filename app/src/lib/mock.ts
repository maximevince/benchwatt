import type { Backend, Command, Events, Device, DeviceDesc, DeviceState, ChannelDesc, Monitor, RecordingInfo, View, AllStats, UartChunk, Edge, Stats, OutputDesc, SettingDesc } from './backend';
import { settingError } from './channels';

// Browser stand-in for the Tauri backend: simulates concrete devices through the same contract.
// ?devices=arc,gpp,ppk2 picks the devices (default: one Arc); ?disconnected simulates none.
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
      outputs: [{ n: 1, label: 'Main output', group: 'main', voltage: { min: 0, max: 3.76 }, voltage_steps: [], current_limit: { min: 0.001, max: 3 } }] },
    state: { outputs: [{ enabled: false, voltage: 3.3, current_limit: 0.3 }], settings: { current_mode: 'cutoff', io_voltage: 1.9, '5v_pin': false, '4wire': false }, gpo: [false, false], uart_open: false },
    phase: 0,
    monitor(sim) {
      const out = sim.state.outputs[0], limitMode = sim.state.settings.current_mode === 'limit';
      // A short burst lasts one sample at the 10 Hz monitor rate, representing a 5 ms load event.
      if (sim.phase % 97 === 0) spike = 1;
      const voltage = out.enabled ? out.voltage + Math.sin(sim.phase / 27) * 0.0015 : 0;
      let current = (out.enabled ? voltage / 100 + (Math.random() - 0.5) * 0.00035 : 0) * (spike ? 3 : 1);
      spike = 0;
      if (out.enabled && current > out.current_limit) {
        if (!limitMode) {
          sim.state = { ...sim.state, outputs: [{ ...out, enabled: false }] };
          emit('alert', { device: sim.desc.id, id: 6, name: 'main-overcurr' });
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
  const limits = [[32, 3], [32, 3], [5, 1], [15, 1]], loads = [50, 100, 10, 200];
  const outputs: OutputDesc[] = limits.map(([volts, amps], i) => ({ n: i + 1, label: `CH${i + 1}`, group: `ch${i + 1}`, voltage: { min: 0, max: volts }, voltage_steps: [], current_limit: { min: 0, max: amps } }));
  const channels = outputs.flatMap(out => (['v', 'i', 'p'] as const).map(q => channel(`${out.group}.${q}`, `${out.label} ${q === 'v' ? 'voltage' : q === 'i' ? 'current' : 'power'}`, q === 'v' ? 'V' : q === 'i' ? 'A' : 'W', 'Analog', q === 'v' ? 'Voltage' : q === 'i' ? 'Current' : 'Power', 10, { group: out.group })));
  const settings: SettingDesc[] = outputs.flatMap(out => [
    { key: `ovp${out.n}`, label: 'OVP', type: 'number' as const, min: 0, max: out.voltage.max + 1, unit: 'V', output: out.n, readonly: false },
    { key: `ocp${out.n}`, label: 'OCP', type: 'number' as const, min: 0, max: out.current_limit!.max + 0.1, unit: 'A', output: out.n, readonly: false },
  ]);
  const load = (sim: Sim, i: number) => { const out = sim.state.outputs[i]; return out.enabled ? Math.min(out.voltage / loads[i], out.current_limit) : 0; };
  const value = (sim: Sim, ch: string, noise: number) => {
    const i = Number(ch.slice(2, 3)) - 1, out = sim.state.outputs[i], amps = load(sim, i) * (1 + noise), volts = out.enabled ? (amps >= out.current_limit && out.current_limit > 0 ? amps * loads[i] : out.voltage) : 0;
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
  const current = (sim: Sim, t: number) => sim.state.settings.mode === 'ampere' || sim.state.outputs[0].enabled ? (awake(t, 0.5, 0.01) ? 0.015 : 0.000003) * sim.state.outputs[0].voltage / 3 : 0;
  return {
    desc: { id, kind: 'ppk2', model: 'PPK2', serial: 'C2A1F7E5', firmware: '1.2.1', hardware: '2', channels, gpo: 0, uart: false,
      outputs: [{ n: 1, label: 'Source', group: 'out', voltage: { min: 0.8, max: 5 }, voltage_steps: [], current_limit: null }],
      settings: [{ key: 'mode', label: 'Mode', type: 'choice', options: ['source', 'ampere'], output: null, readonly: false }] },
    state: { outputs: [{ enabled: false, voltage: 3, current_limit: 0 }], settings: { mode: 'source' }, gpo: [], uart_open: false },
    phase: 0,
    monitor: sim => ({ readings: { i: current(sim, sim.phase / 10 + Math.random() * 0.5), v: sim.state.outputs[0].voltage }, inputs: (sim.phase >> 3) & 0xff }),
    sample: (sim, ch, t) => ch === 'i' ? current(sim, t) : sim.state.outputs[0].voltage,
    edges: (_sim, t) => [{ t, mask: Math.round(t * 10) & 0xff }],
  };
}

const makers: Record<string, (id: string) => Sim> = { arc, gpp, ppk2 };
const sims: Sim[] = [];
let connected = false;
for (const kind of kinds) {
  if (!makers[kind]) continue;
  const count = sims.filter(s => s.desc.kind === kind).length;
  sims.push(makers[kind](count ? `${kind}-${count + 1}` : kind));
}
const deviceList = (): Device[] => connected ? sims.map(s => ({ desc: s.desc, state: { ...s.state } })) : [];
function sim(id: unknown): Sim {
  return (connected && sims.find(s => s.desc.id === id)) || fail(`no device ${String(id)}`);
}

let recording: RecordingInfo | null = null;
let recordingSim: Sim | null = null;
let recordingTick = 0, pulse = 0;
let uartLog: UartChunk[] = [];
let edges: Edge[] = [];
const values: Record<string, number[]> = {};
const pyramid: Record<string, { min: number; max: number }[][]> = {};
const rate = (ch: string) => recording?.channels.find(c => c.name === ch)?.rate_hz ?? 0;
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
/** Appends 100 ms of samples from the recorded device. */
function generate(source: Sim, info: RecordingInfo, uartOpen: boolean) {
  const start = recordingTick / 10;
  for (const ch of info.channels) {
    if (ch.kind === 'Digital' && source.edges) { edges.push(...source.edges(source, start)); info.samples[ch.name] = edges.length; }
    else if (ch.kind === 'Bytes' && !ch.set && uartOpen && recordingTick % 10 === 0) {
      uartLog.push({ t: start, dir: 'rx', bytes: Array.from(new TextEncoder().encode(`tick ${++pulse}\n`)) });
      info.samples[ch.name] = uartLog.filter(x => x.dir === 'rx').reduce((n, x) => n + x.bytes.length, 0);
    } else if (ch.kind === 'Analog') {
      for (let i = 0; i < ch.rate_hz / 10; i++) add(ch.name, source.sample(source, ch.name, start + i / ch.rate_hz));
      info.samples[ch.name] = values[ch.name].length;
    }
  }
  recordingTick++;
}
function recTick() {
  if (!recording?.active || !recordingSim) return;
  generate(recordingSim, recording, recordingSim.state.uart_open);
  recording = { ...recording, duration_s: recordingTick / 10, samples: { ...recording.samples } };
  emit('recording', recording);
}
window.setInterval(recTick, 100);
function reset() {
  for (const key of Object.keys(values)) delete values[key];
  for (const key of Object.keys(pyramid)) delete pyramid[key];
  edges = []; uartLog = []; pulse = 0; recordingTick = 0;
}
function mockView(ch: string, t0: number, t1: number, px: number): View {
  const r = rate(ch), raw = values[ch] ?? [];
  if (!r) return { kind: 'raw', t: [], v: [] };
  const lo = Math.max(0, Math.ceil(t0 * r)), hi = Math.min(raw.length, Math.floor(t1 * r) + 1);
  if (hi - lo <= 2 * px) {
    const t: number[] = [], v: number[] = [];
    for (let i = lo; i < hi; i++) { t.push(i / r); v.push(raw[i]); }
    return { kind: 'raw', t, v };
  }
  const levels = pyramid[ch];
  let level = 1;
  while (level + 1 < levels.length && (hi - lo) / 16 ** level > 2 * px) level++;
  const size = 16 ** level, bins = levels[level] ?? [];
  const t: number[] = [], min: number[] = [], max: number[] = [];
  for (let i = Math.floor(lo / size); i < Math.min(bins.length, Math.ceil(hi / size)); i++) {
    t.push(i * size / r); min.push(bins[i].min); max.push(bins[i].max);
  }
  const tailStart = bins.length * size;
  if (tailStart < hi && tailStart < raw.length) {
    const tail = raw.slice(Math.max(tailStart, lo), hi);
    if (tail.length) {
      let low = Infinity, high = -Infinity;
      for (const value of tail) { low = Math.min(low, value); high = Math.max(high, value); }
      t.push(tailStart / r); min.push(low); max.push(high);
    }
  }
  return { kind: 'envelope', t, min, max };
}
function mockStats(t0: number, t1: number): AllStats {
  const channels: Record<string, Stats> = {};
  for (const [ch, raw] of Object.entries(values)) {
    const r = rate(ch), slice = raw.slice(Math.max(0, Math.ceil(t0 * r)), Math.min(raw.length, Math.floor(t1 * r) + 1));
    if (!slice.length) continue;
    let sum = 0, square = 0, min = Infinity, max = -Infinity;
    for (const x of slice) { sum += x; square += x*x; min = Math.min(min, x); max = Math.max(max, x); }
    channels[ch] = { count: slice.length, min, max, mean: sum / slice.length, rms: Math.sqrt(square / slice.length), integral: sum / r };
  }
  const first = (q: string) => recording?.channels.find(c => c.kind === 'Analog' && c.quantity === q && channels[c.name]);
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
  for (const s of sims) {
    s.phase++;
    emit('monitor', { device: s.desc.id, ...s.monitor(s) });
  }
}
if (sims.length) window.setTimeout(() => {
  connected = true;
  emit('devices', deviceList());
  tick();
  window.setInterval(tick, 100);
}, 1000);
/** Byte channels of the recording, in description order, including those now in use. */
function withChannels(names: string[]): ChannelDesc[] {
  return recordingSim!.desc.channels.filter(ch => names.includes(ch.name));
}
/** A recording to open when none was made in this session: 5 s of the Arc with its UART. */
function cannedRecording(): RecordingInfo {
  const source = arc('arc'), names = ['mc', 'mv', 'mp', 'gpi', 'rx'];
  reset();
  const info: RecordingInfo = { active: false, duration_s: 5, samples: {}, gaps: 0, channels: source.desc.channels.filter(ch => names.includes(ch.name)), device: null };
  recording = info;
  for (let i = 0; i < 50; i++) generate(source, info, true);
  return info;
}
export const mockBackend: Backend = {
  getDevices: async () => deviceList(),
  command: async (name, args) => apply(name, args),
  listen: async (name, handler) => {
    const set = listeners.get(name) ?? new Set();
    set.add(handler as (value: never) => void);
    listeners.set(name, set);
    return () => { set.delete(handler as (value: never) => void); };
  },
  startRecording: async (device, channels) => {
    const s = sim(device);
    for (const name of channels) if (!s.desc.channels.some(ch => ch.name === name)) fail(`unknown channel ${name}`);
    reset();
    recordingSim = s;
    const rx = s.desc.channels.find(ch => ch.kind === 'Bytes' && !ch.set);
    const selected = withChannels([...channels, ...(s.state.uart_open && rx ? [rx.name] : [])]);
    recording = { active: true, duration_s: 0, samples: Object.fromEntries(selected.map(ch => [ch.name, 0])), gaps: 0, channels: selected, device: s.desc.id };
    emit('recording', recording);
    return recording;
  },
  stopRecording: async () => { if (recording) { recording = { ...recording, active: false }; emit('recording', recording); } return recording; },
  recordingInfo: async () => recording,
  getView: async (ch, t0, t1, px) => mockView(ch, t0, t1, px),
  getStats: async (t0, t1) => mockStats(t0, t1),
  getUart: async (t0, t1) => uartLog.filter(x => x.t >= t0 && x.t <= t1),
  saveRecording: async () => {},
  loadRecording: async () => { recording = recording ? { ...recording, active: false, device: null } : cannedRecording(); recordingSim = null; emit('recording', recording); return recording; },
  exportRecording: async () => {},
  getEdges: async (t0, t1) => edges.filter(x => x.t >= t0 && x.t <= t1),
  uartOpen: async (device, baud) => {
    const s = sim(device);
    if (!s.desc.uart) fail('device has no UART');
    if (!Number.isInteger(baud) || baud <= 0) fail('invalid baud rate');
    s.state = { ...s.state, uart_open: true }; changed(s);
    const rx = s.desc.channels.find(ch => ch.kind === 'Bytes' && !ch.set);
    if (recording?.active && recordingSim === s && rx && !recording.channels.includes(rx)) recording = { ...recording, channels: withChannels([...recording.channels.map(ch => ch.name), rx.name]), samples: { ...recording.samples, [rx.name]: 0 } };
  },
  uartClose: async device => { const s = sim(device); s.state = { ...s.state, uart_open: false }; changed(s); },
  uartSend: async (device, text) => {
    const s = sim(device);
    if (!s.state.uart_open) fail('UART closed');
    if (!recording?.active || recordingSim !== s) return;
    const tx = s.desc.channels.find(ch => ch.kind === 'Bytes' && ch.set), rx = s.desc.channels.find(ch => ch.kind === 'Bytes' && !ch.set);
    const t = recording.duration_s, bytes = Array.from(new TextEncoder().encode(text));
    uartLog.push({ t, dir: 'tx', bytes });
    if (tx) recording = { ...recording, channels: withChannels([...recording.channels.map(ch => ch.name), tx.name]), samples: { ...recording.samples, [tx.name]: (recording.samples[tx.name] ?? 0) + bytes.length } };
    window.setTimeout(() => {
      if (!recording?.active || !rx) return;
      // The device splits the echo over 1 ms packets; the recorder joins them back into one chunk.
      uartLog.push({ t: t + .001, dir: 'rx', bytes });
      recording = { ...recording, samples: { ...recording.samples, [rx.name]: (recording.samples[rx.name] ?? 0) + bytes.length } };
      emit('recording', recording);
    }, 1);
    emit('recording', recording);
  },
};
