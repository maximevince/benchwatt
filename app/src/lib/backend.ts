import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { mockBackend } from './mock';

export type DeviceInfo = { name: string | null; fw: string | null; hw: string | null; uid: string | null };
export type Connection = { connected: boolean; info?: DeviceInfo };
export type Settings = { main: boolean | null; mainvolt: number | null; maxcur: number | null; mainsrccurrmode: number | null; mainvolt_max: number | null; expvolt: number | null; gpo: (boolean | null)[]; fourwire: boolean | null };
export type Monitor = { mv: number | null; mc: number | null; mp: number | null; vb: number | null; vj: number | null; tp: number | null; gpi: number | null; fourwire: boolean | null };
export type Alert = { id: number; name: string };
export type Events = { connection: Connection; settings: Settings; monitor: Monitor; alert: Alert };
export type Command = 'set_output' | 'set_voltage' | 'set_current_limit' | 'set_current_mode' | 'set_gpo';
export type Snapshot = { connection: Connection; settings: Settings | null };
export interface Backend {
  getState(): Promise<Snapshot>;
  command(name: Command, args: Record<string, unknown>): Promise<Settings>;
  listen<K extends keyof Events>(name: K, handler: (event: Events[K]) => void): Promise<() => void>;
}
const tauriBackend: Backend = {
  getState: () => invoke<Snapshot>('get_state'),
  command: (name, args) => invoke<Settings>(name, args),
  listen: (name, handler) => listen<Events[typeof name]>(name, event => handler(event.payload)),
};
export const backend: Backend = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window ? tauriBackend : mockBackend;
