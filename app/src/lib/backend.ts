import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { mockBackend } from './mock';
import { decodeView, type View } from './view';
export { decodeView } from './view';
export type { View } from './view';

export type Quantity = 'Current' | 'Voltage' | 'Power' | 'Temperature' | 'Other';
export type ChannelDesc = { id: number; name: string; label: string; unit: string; rate_hz: number; kind: 'Analog' | 'Digital' | 'Bytes'; quantity: Quantity; group: string; bits: number; extra: boolean; set: boolean };
export type Range = { min: number; max: number };
export type OutputDesc = { n: number; label: string; group: string; voltage: Range; voltage_steps: number[]; current_limit: Range | null; color: string | null };
export type SettingValue = boolean | string | number;
export type SettingDesc = { key: string; label: string; type: 'bool' | 'choice' | 'number'; options?: string[]; min?: number; max?: number; unit?: string; output: number | null; readonly: boolean };
export type DeviceDesc = { id: string; kind: string; model: string; serial: string; firmware: string; hardware: string; channels: ChannelDesc[]; outputs: OutputDesc[]; settings: SettingDesc[]; gpo: number; uart: boolean };
export type OutputState = { enabled: boolean; voltage: number | null; current_limit: number | null };
export type DeviceState = { outputs: OutputState[]; settings: Record<string, SettingValue | null>; gpo: (boolean | null)[]; uart_open: boolean };
export type Device = { desc: DeviceDesc; state: DeviceState | null };
/** A supported instrument that is plugged in. Its key is the id of the device once it is open. */
export type FoundDevice = { key: string; kind: string; name: string; port: string };
/** What is plugged in, which device the user chose, and that device once it is open. */
export type Devices = { found: FoundDevice[]; wanted: string | null; active: Device | null; error: string | null };
export type Source = { kind: string | null; model: string | null; serial: string | null; firmware: string | null; hardware: string | null };
export type Monitor = { device: string; readings: Record<string, number>; inputs: number | null };
export type Alert = { device: string; id: number; name: string; title: string; detail: string; severity: 'warning' | 'error' };
export type RecordingInfo = { id: number; active: boolean; duration_s: number; samples: Record<string, number>; gaps: number; channels: ChannelDesc[]; device: string | null; source: Source };
export type Stats = { count: number; min: number; max: number; mean: number; rms: number; integral: number };
export type AllStats = { channels: Record<string, Stats>; charge_mah: number | null; energy: { joules: number; mwh: number; average_power: number } | null };
export type UartChunk = { t: number; bytes: number[]; dir: 'rx' | 'tx' };
export type Edge = { t: number; mask: number };
export type Events = { devices: Devices; state: { device: string; state: DeviceState }; monitor: Monitor; alert: Alert; recording: RecordingInfo };
/** Commands that change a device and answer with its new state. */
export type Command = 'set_output' | 'set_voltage' | 'set_current_limit' | 'set_setting' | 'set_gpo';
export interface Backend {
  getDevices(): Promise<Devices>;
  /** Chooses the device to work with, or none. The result arrives as a `devices` event. */
  selectDevice(device: string | null): Promise<void>;
  command(name: Command, args: { device: string } & Record<string, unknown>): Promise<DeviceState>;
  listen<K extends keyof Events>(name: K, handler: (event: Events[K]) => void): Promise<() => void>;
  /** Every open recording. Each has an id that the calls below take as `rec`. */
  recordings(): Promise<RecordingInfo[]>;
  startRecording(device: string, channels: string[]): Promise<RecordingInfo>;
  stopRecording(): Promise<void>;
  closeRecording(rec: number): Promise<void>;
  getView(rec: number, ch: string, t0: number, t1: number, px: number): Promise<View>;
  getStats(rec: number, t0: number, t1: number): Promise<AllStats>;
  getUart(rec: number, t0: number, t1: number): Promise<UartChunk[]>;
  getEdges(rec: number, t0: number, t1: number): Promise<Edge[]>;
  saveRecording(rec: number, path: string): Promise<void>;
  loadRecording(path: string): Promise<RecordingInfo>;
  exportRecording(rec: number, path: string, t0?: number, t1?: number): Promise<void>;
  uartOpen(device: string, baud: number): Promise<void>;
  uartClose(device: string): Promise<void>;
  uartSend(device: string, text: string): Promise<void>;
}
const tauriBackend: Backend = {
  getDevices: () => invoke('get_devices'),
  selectDevice: device => invoke('select_device', { device }),
  command: (name, args) => invoke<DeviceState>(name, args),
  listen: (name, handler) => listen<Events[typeof name]>(name, event => handler(event.payload)),
  recordings: () => invoke('recordings'),
  startRecording: (device, channels) => invoke('start_recording', { device, channels }),
  stopRecording: () => invoke('stop_recording'),
  closeRecording: rec => invoke('close_recording', { rec }),
  getView: async (rec, ch, t0, t1, px) => decodeView(new Uint8Array(await invoke<ArrayBuffer>('get_view', { rec, ch, t0, t1, px }))),
  getStats: (rec, t0, t1) => invoke('get_stats', { rec, t0, t1 }),
  getUart: (rec, t0, t1) => invoke('get_uart', { rec, t0, t1 }),
  getEdges: (rec, t0, t1) => invoke('get_edges', { rec, t0, t1 }),
  saveRecording: (rec, path) => invoke('save_recording', { rec, path }),
  loadRecording: path => invoke('load_recording', { path }),
  exportRecording: (rec, path, t0, t1) => invoke('export_recording', { rec, path, t0, t1 }),
  uartOpen: (device, baud) => invoke('uart_open', { device, baud }),
  uartClose: device => invoke('uart_close', { device }),
  uartSend: (device, text) => invoke('uart_send', { device, text }),
};
export const backend: Backend = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window ? tauriBackend : mockBackend;
