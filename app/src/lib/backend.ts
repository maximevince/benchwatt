import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { mockBackend } from './mock';
import { decodeView, type View } from './view';
export { decodeView } from './view';
export type { View } from './view';

export type Quantity = 'Current' | 'Voltage' | 'Power' | 'Temperature' | 'Other';
export type ChannelDesc = { id: number; name: string; label: string; unit: string; rate_hz: number; kind: 'Analog' | 'Digital' | 'Bytes'; quantity: Quantity; group: string; bits: number; extra: boolean; set: boolean };
export type Range = { min: number; max: number };
export type OutputDesc = { n: number; label: string; group: string; voltage: Range; voltage_steps: number[]; current_limit: Range | null };
export type SettingValue = boolean | string | number;
export type SettingDesc = { key: string; label: string; type: 'bool' | 'choice' | 'number'; options?: string[]; min?: number; max?: number; unit?: string; output: number | null; readonly: boolean };
export type DeviceDesc = { id: string; kind: string; model: string; serial: string; firmware: string; hardware: string; channels: ChannelDesc[]; outputs: OutputDesc[]; settings: SettingDesc[]; gpo: number; uart: boolean };
export type OutputState = { enabled: boolean; voltage: number | null; current_limit: number | null };
export type DeviceState = { outputs: OutputState[]; settings: Record<string, SettingValue | null>; gpo: (boolean | null)[]; uart_open: boolean };
export type Device = { desc: DeviceDesc; state: DeviceState | null };
export type Monitor = { device: string; readings: Record<string, number>; inputs: number | null };
export type Alert = { device: string; id: number; name: string; title: string; detail: string; severity: 'warning' | 'error' };
export type RecordingInfo = { active: boolean; duration_s: number; samples: Record<string, number>; gaps: number; channels: ChannelDesc[]; device: string | null };
export type Stats = { count: number; min: number; max: number; mean: number; rms: number; integral: number };
export type AllStats = { channels: Record<string, Stats>; charge_mah: number | null; energy: { joules: number; mwh: number; average_power: number } | null };
export type UartChunk = { t: number; bytes: number[]; dir: 'rx' | 'tx' };
export type Edge = { t: number; mask: number };
export type Events = { devices: Device[]; state: { device: string; state: DeviceState }; monitor: Monitor; alert: Alert; recording: RecordingInfo };
/** Commands that change a device and answer with its new state. */
export type Command = 'set_output' | 'set_voltage' | 'set_current_limit' | 'set_setting' | 'set_gpo';
export interface Backend {
  getDevices(): Promise<Device[]>;
  command(name: Command, args: { device: string } & Record<string, unknown>): Promise<DeviceState>;
  listen<K extends keyof Events>(name: K, handler: (event: Events[K]) => void): Promise<() => void>;
  startRecording(device: string, channels: string[]): Promise<RecordingInfo>;
  stopRecording(): Promise<RecordingInfo | null>;
  recordingInfo(): Promise<RecordingInfo | null>;
  getView(ch: string, t0: number, t1: number, px: number): Promise<View>;
  getStats(t0: number, t1: number): Promise<AllStats>;
  getUart(t0: number, t1: number): Promise<UartChunk[]>;
  saveRecording(path: string): Promise<void>;
  loadRecording(path: string): Promise<RecordingInfo>;
  exportRecording(path: string, t0?: number, t1?: number): Promise<void>;
  getEdges(t0: number, t1: number): Promise<Edge[]>;
  uartOpen(device: string, baud: number): Promise<void>;
  uartClose(device: string): Promise<void>;
  uartSend(device: string, text: string): Promise<void>;
}
const tauriBackend: Backend = {
  getDevices: () => invoke('get_devices'),
  command: (name, args) => invoke<DeviceState>(name, args),
  listen: (name, handler) => listen<Events[typeof name]>(name, event => handler(event.payload)),
  startRecording: (device, channels) => invoke('start_recording', { device, channels }),
  stopRecording: () => invoke('stop_recording'),
  recordingInfo: () => invoke('recording_info'),
  getView: async (ch, t0, t1, px) => decodeView(new Uint8Array(await invoke<ArrayBuffer>('get_view', { ch, t0, t1, px }))),
  getStats: (t0, t1) => invoke('get_stats', { t0, t1 }),
  getUart: (t0, t1) => invoke('get_uart', { t0, t1 }),
  saveRecording: path => invoke('save_recording', { path }),
  loadRecording: path => invoke('load_recording', { path }),
  exportRecording: (path,t0,t1) => invoke('export_recording', { path,t0,t1 }),
  getEdges: (t0, t1) => invoke('get_edges', { t0, t1 }),
  uartOpen: (device, baud) => invoke('uart_open', { device, baud }),
  uartClose: device => invoke('uart_close', { device }),
  uartSend: (device, text) => invoke('uart_send', { device, text }),
};
export const backend: Backend = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window ? tauriBackend : mockBackend;
