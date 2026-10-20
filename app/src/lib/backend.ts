import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { mockBackend } from './mock';
import { decodeView, type View } from './view';
export { decodeView } from './view';
export type { View } from './view';

export type DeviceInfo = { name: string | null; fw: string | null; hw: string | null; uid: string | null; proto: number };
export type Connection = { connected: boolean; info?: DeviceInfo };
export type Settings = { main: boolean | null; mainvolt: number | null; maxcur: number | null; mainsrccurrmode: number | null; mainvolt_max: number | null; expvolt: number | null; five_volt: boolean | null; gpo: (boolean | null)[]; fourwire: boolean | null };
export type Monitor = { mv: number | null; mc: number | null; mp: number | null; vb: number | null; vj: number | null; tp: number | null; gpi: number | null; fourwire: boolean | null };
export type Alert = { id: number; name: string };
export type RecordingInfo = { active: boolean; duration_s: number; samples: Record<string, number>; gaps: number; channels: string[] };
export type Stats = { count: number; min: number; max: number; mean: number; rms: number; integral: number };
export type AllStats = { channels: Record<string, Stats>; charge_mah: number | null; energy: { joules: number; mwh: number; average_power: number } | null };
export type UartChunk = { t: number; bytes: number[]; dir: "rx" | "tx" };
export type Edge = { t: number; mask: number };
export type Events = { connection: Connection; settings: Settings; monitor: Monitor; alert: Alert; recording: RecordingInfo };
export type Command = 'set_output' | 'set_voltage' | 'set_current_limit' | 'set_current_mode' | 'set_gpo' | 'set_exp_voltage' | 'set_5v';
export type Snapshot = { connection: Connection; settings: Settings | null };
export interface Backend {
  getState(): Promise<Snapshot>;
  command(name: Command, args: Record<string, unknown>): Promise<Settings>;
  listen<K extends keyof Events>(name: K, handler: (event: Events[K]) => void): Promise<() => void>;
  startRecording(channels: string[]): Promise<RecordingInfo>;
  stopRecording(): Promise<RecordingInfo | null>;
  recordingInfo(): Promise<RecordingInfo | null>;
  getView(ch: string, t0: number, t1: number, px: number): Promise<View>;
  getStats(t0: number, t1: number): Promise<AllStats>;
  getUart(t0: number, t1: number): Promise<UartChunk[]>;
  saveRecording(path: string): Promise<void>;
  loadRecording(path: string): Promise<RecordingInfo>;
  exportRecording(path: string, t0?: number, t1?: number): Promise<void>;
  getEdges(t0: number, t1: number): Promise<Edge[]>;
  uartOpen(baud: number): Promise<void>;
  uartClose(): Promise<void>;
  uartSend(text: string): Promise<void>;
}
const tauriBackend: Backend = {
  getState: () => invoke<Snapshot>('get_state'),
  command: (name, args) => invoke<Settings>(name, args),
  listen: (name, handler) => listen<Events[typeof name]>(name, event => handler(event.payload)),
  startRecording: channels => invoke('start_recording', { channels }),
  stopRecording: () => invoke('stop_recording'),
  recordingInfo: () => invoke('recording_info'),
  getView: async (ch, t0, t1, px) => decodeView(new Uint8Array(await invoke<ArrayBuffer>('get_view', { ch, t0, t1, px }))),
  getStats: (t0, t1) => invoke('get_stats', { t0, t1 }),
  getUart: async (t0, t1) => { const [rx, tx] = await Promise.all([invoke<Omit<UartChunk, 'dir'>[]>('get_uart', { ch: 23, t0, t1 }), invoke<Omit<UartChunk, 'dir'>[]>('get_uart', { ch: 25, t0, t1 })]); return [...rx.map(x => ({ ...x, dir: 'rx' as const })), ...tx.map(x => ({ ...x, dir: 'tx' as const }))].sort((a,b) => a.t-b.t); },
  saveRecording: path => invoke('save_recording', { path }),
  loadRecording: path => invoke('load_recording', { path }),
  exportRecording: (path,t0,t1) => invoke('export_recording', { path,t0,t1 }),
  getEdges: (t0, t1) => invoke('get_edges', { t0, t1 }),
  uartOpen: baud => invoke('uart_open', { baud }),
  uartClose: () => invoke('uart_close'),
  uartSend: text => invoke('uart_send', { text }),
};
export const backend: Backend = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window ? tauriBackend : mockBackend;
