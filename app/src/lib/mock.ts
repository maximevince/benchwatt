import type { Backend, Command, Events, Settings, Monitor, Connection } from './backend';

const disconnected = new URLSearchParams(window.location.search).has('disconnected');
let connection: Connection = { connected: false };
let settings: Settings = { main: false, mainvolt: 3.3, maxcur: 0.3, mainsrccurrmode: 0, mainvolt_max: 3.76, expvolt: 3.3, gpo: [false, false, false], fourwire: false };
const listeners = new Map<keyof Events, Set<(value: never) => void>>();
function emit<K extends keyof Events>(name: K, value: Events[K]) {
  for (const handler of listeners.get(name) ?? []) handler(value as never);
}
let phase = 0;
let spike = 0;
function tick() {
  if (!connection.connected) return;
  phase++;
  // A short burst lasts one sample at the 10 Hz monitor rate, representing a 5 ms load event.
  if (phase % 97 === 0) spike = 1;
  const voltage = settings.main ? (settings.mainvolt ?? 0) + Math.sin(phase / 27) * 0.0015 : 0;
  const baseCurrent = settings.main ? voltage / 100 + (Math.random() - 0.5) * 0.00035 : 0;
  let current = baseCurrent * (spike ? 10 : 1);
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
};
