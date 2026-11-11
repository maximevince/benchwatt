import { toast } from 'svelte-sonner';
import { open, save } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { documentDir, join } from '@tauri-apps/api/path';
import { backend, type Alert, type Command, type Device, type DeviceState, type Monitor, type RecordingInfo, type AllStats, type UartChunk, type Edge } from './backend';
import { defaultRecordChannels, lanesFor, orderedEntries, rxChannel } from './channels';

// File dialogs open in the folder used last (per machine), else ~/Documents.
const LAST_DIR_KEY = 'benchwatt.lastDir';
async function inLastDir(name: string): Promise<string> {
  let dir: string | null = null;
  try { dir = localStorage.getItem(LAST_DIR_KEY); } catch { /* storage unavailable */ }
  try { return await join(dir ?? await documentDir(), name); } catch { return name; }
}
function rememberDir(path: string) {
  const end = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  if (end > 0) try { localStorage.setItem(LAST_DIR_KEY, path.slice(0, end)); } catch { /* storage unavailable */ }
}

export type AlertEntry = Alert & { key: number; time: string };
class AppState {
  devices = $state<Device[]>([]);
  monitors = $state<Record<string, Monitor>>({});
  selectedId = $state<string | null>(null);
  /** The device whose panels the sidebar shows: the chosen one while it is present, else the first. */
  selected = $derived(this.devices.find(d => d.desc.id === this.selectedId) ?? this.devices[0] ?? null);
  monitor = $derived(this.selected ? this.monitors[this.selected.desc.id] ?? null : null);
  /** Record channel choices per device id; a device without a choice records its defaults. */
  recordChoice = $state<Record<string, string[]>>({});
  alerts = $state<AlertEntry[]>([]);
  busy = $state(false);
  error = $state('');
  theme = $state<'system' | 'dark' | 'light'>('system');
  systemDark = $state(false);
  unsaved = $state(false);
  closingApproved = false;
  fileName = $state('');
  dialog = $state<'discard' | 'export' | 'shortcuts' | 'about' | 'device' | null>(null);
  private dialogResolve?: (value: boolean) => void;
  marker = $state<number | null>(null);
  hoverMarker = $state<number | null>(null);
  focusedUartTime = $state<number | null>(null);
  follow = $state(true);
  drawerTab = $state('alerts');
  showPower = $state(false);
  drawerOpen = $state(true);
  recording = $state<RecordingInfo | null>(null);
  get recordChannels(): string[] { const d = this.selected; return d ? this.recordChoice[d.desc.id] ?? defaultRecordChannels(d.desc) : []; }
  set recordChannels(channels: string[]) { const d = this.selected; if (d) this.recordChoice = { ...this.recordChoice, [d.desc.id]: channels }; }
  /** Outputs of the device the recording comes from, while it is connected (their colours and names tag the channels). */
  recordingOutputs = $derived(this.devices.find(d => d.desc.id === this.recording?.device)?.desc.outputs ?? []);
  /** Plot lanes of the current recording, from its own channel list. */
  lanes = $derived(lanesFor(this.recording?.channels ?? [], this.recordingOutputs));
  hiddenLanes = $state<string[]>([]);
  range = $state<[number, number] | null>(null);
  viewRange = $state<[number, number]>([0, 10]);
  stats = $state<AllStats | null>(null);
  uart = $state<UartChunk[]>([]);
  edges = $state<Edge[]>([]);
  get uartOpen() { return this.selected?.state?.uart_open ?? false; }
  private key = 0;
  private detailsSeq = 0;
  private statsSeq = 0;
  get darkTheme() { return this.theme === 'dark' || (this.theme === 'system' && this.systemDark); }
  get disabled() { return !this.selected?.state || this.busy; }
  initTheme() {
    const saved = localStorage.getItem('benchwatt-theme');
    this.theme = saved === 'light' || saved === 'dark' ? saved : 'system';
    this.systemDark = matchMedia('(prefers-color-scheme: dark)').matches;
    this.applyTheme();
  }
  showShortcuts() { this.dialog = 'shortcuts'; }
  toggleTheme() { this.setTheme(this.darkTheme ? 'light' : 'dark'); }
  setTheme(theme: 'system' | 'light' | 'dark') { this.theme = theme; localStorage.setItem('benchwatt-theme', theme); this.applyTheme(); }
  private applyTheme() { document.documentElement.classList.toggle('dark', this.darkTheme); }
  async start(): Promise<() => void> {
    this.initTheme();
    const media = matchMedia('(prefers-color-scheme: dark)');
    const changed = (event: MediaQueryListEvent) => { this.systemDark = event.matches; if (this.theme === 'system') this.applyTheme(); };
    media.addEventListener('change', changed);
    const off = await Promise.all([
      backend.listen('devices', value => this.setDevices(value)),
      backend.listen('state', value => this.setState(value.device, value.state)),
      backend.listen('monitor', value => { if (this.devices.some(d => d.desc.id === value.device)) this.monitors = { ...this.monitors, [value.device]: value }; }),
      backend.listen('alert', value => {
        const entry = { ...value, key: ++this.key, time: new Date().toLocaleTimeString() };
        this.alerts = [entry, ...this.alerts].slice(0, 100);
        const message = `${value.title}: ${value.detail}`;
        if (value.severity === 'warning') toast.warning(message);
        else toast.error(message);
      }),
      backend.listen('recording', value => { if (this.recording?.active && this.fileName && value.duration_s > this.recording.duration_s) this.unsaved = true; this.recording = value; }),
    ]);
    try {
      this.setDevices(await backend.getDevices());
      this.recording = await backend.recordingInfo();
    } catch (cause) { this.report(cause); }
    return () => { off.forEach(fn => fn()); media.removeEventListener('change', changed); };
  }
  private setDevices(devices: Device[]) {
    this.devices = devices;
    const ids = new Set(devices.map(d => d.desc.id));
    this.monitors = Object.fromEntries(Object.entries(this.monitors).filter(([id]) => ids.has(id)));
    if (this.selectedId && !ids.has(this.selectedId)) this.selectedId = null;
  }
  private setState(id: string, state: DeviceState) { this.devices = this.devices.map(d => d.desc.id === id ? { ...d, state } : d); }
  select(id: string) { this.selectedId = id; }
  /** Applies a command to the selected device. */
  async command(name: Command, args: Record<string, unknown>) {
    const device = this.selected?.desc.id;
    if (this.disabled || !device) return false;
    this.busy = true; this.error = '';
    try { this.setState(device, await backend.command(name, { ...args, device })); return true; }
    catch (cause) { this.report(cause); return false; }
    finally { this.busy = false; }
  }
  report(cause: unknown) { this.error = String(cause); toast.error(this.error); }
  /** Toggles the first output of the selected device. */
  async toggleOutput() {
    const on = !this.selected?.state?.outputs[0]?.enabled, out = this.selected?.desc.outputs[0]?.n;
    return out !== undefined && await this.command('set_output', { out, on }) ? on : null;
  }
  async toggleRecording() {
    try {
      if (this.recording?.active) { this.recording = await backend.stopRecording(); return; }
      const device = this.selected;
      if (!device) return;
      if (!await this.confirmDiscard()) return;
      // A device may only stream received UART bytes with its UART open, so recording them opens it.
      const rx = rxChannel(device.desc.channels);
      if (device.desc.uart && rx && this.recordChannels.includes(rx.name) && !this.uartOpen) await this.toggleUart();
      this.recording = await backend.startRecording(device.desc.id, this.recordChannels);
      this.hiddenLanes = [];
      this.range = null;
      this.viewRange = [0, 10];
      this.stats = null;
      this.uart = [];
      this.edges = [];
      this.fileName = ''; this.unsaved = true;
    } catch (cause) { this.report(cause); }
  }
  async confirmDiscard(): Promise<boolean> {
    if (!this.recording || !this.unsaved) return true;
    this.dialog = 'discard';
    return new Promise(resolve => { this.dialogResolve = resolve; });
  }
  resolveDialog(value: boolean) { this.dialogResolve?.(value); this.dialogResolve = undefined; this.dialog = null; }
  private mock() { return !('__TAURI_INTERNALS__' in window); }
  async saveRecording() {
    if (!this.recording) return;
    try { const path = this.mock() ? 'mock.bwrec' : await save({ defaultPath: await inLastDir(this.fileName || 'recording.bwrec'), filters: [{ name: 'Benchwatt recording', extensions: ['bwrec'] }] });
      if (!path) return; rememberDir(path); await backend.saveRecording(path); this.fileName = path.split(/[\\/]/).pop() ?? path; this.unsaved = false; toast.success(this.mock() ? 'Mock recording saved' : 'Recording saved');
    } catch (cause) { this.report(cause); }
  }
  async openRecording() {
    if (!await this.confirmDiscard()) return;
    try { const path = this.mock() ? 'mock.bwrec' : await open({ multiple: false, defaultPath: await inLastDir(''), filters: [{ name: 'Benchwatt recording', extensions: ['bwrec'] }] });
      if (typeof path !== 'string') return; rememberDir(path); this.recording = await backend.loadRecording(path); this.fileName = path.split(/[\\/]/).pop() ?? path; this.unsaved = false; this.hiddenLanes = []; this.range = null; this.uart = []; this.edges = []; this.viewRange = [0, Math.max(this.recording.duration_s, .001)]; await this.refreshDetails();
    } catch (cause) { this.report(cause); }
  }
  async exportRecording(extension: 'csv' | 'sr', selected = false) {
    if (!this.recording) return;
    try { const path = this.mock() ? `mock.${extension}` : await save({ defaultPath: await inLastDir(`export.${extension}`), filters: [{ name: extension === 'csv' ? 'CSV' : 'sigrok session', extensions: [extension] }] });
      if (!path) return; rememberDir(path); await backend.exportRecording(path, ...(selected && this.range ? this.range : [])); toast.success(this.mock() ? 'Mock export complete' : 'Export complete');
    } catch (cause) { this.report(cause); }
  }
  async requestExport(extension: 'csv' | 'sr') {
    if (this.range) { this.exportFormat = extension; this.dialog = 'export'; } else await this.exportRecording(extension);
  }
  exportFormat = $state<'csv' | 'sr'>('csv');
  async quit() { if (!await this.confirmDiscard()) return; if (this.mock()) toast.info('Quit is available in the desktop app'); else { this.closingApproved = true; await getCurrentWindow().destroy(); } }
  selectAll() { if (this.recording) { this.range = [0,this.recording.duration_s]; void this.refreshDetails(); } }
  clearSelection() { this.range = null; void this.refreshDetails(); }
  zoom(factor: number) { const [a,b] = this.viewRange; const mid=(a+b)/2, span=Math.max(.000001,(b-a)*factor); this.viewRange=[Math.max(0,mid-span/2), Math.max(span,mid+span/2)]; this.follow=false; }
  zoomFit() { this.viewRange=[0,Math.max(this.recording?.duration_s ?? 0,.001)]; this.follow=false; }
  centerUart(t: number) { const span=this.viewRange[1]-this.viewRange[0]; if (t < this.viewRange[0] || t > this.viewRange[1]) this.viewRange=[Math.max(0,t-span/2),Math.max(span,t+span/2)]; this.marker=t; this.focusedUartTime=t; }
  async toggleUart() {
    const device = this.selected; if (!device?.state || !device.desc.uart) return;
    const id = device.desc.id, open = !device.state.uart_open;
    try {
      if (open) await backend.uartOpen(id, this.baud); else await backend.uartClose(id);
      const state = this.devices.find(d => d.desc.id === id)?.state; if (state) this.setState(id, { ...state, uart_open: open });
      if (open) { this.drawerTab='uart'; this.drawerOpen=true; }
    } catch (cause) { this.report(cause); }
  }
  async uartSend(text: string) { const id = this.selected?.desc.id; if (!id) throw new Error('No device'); await backend.uartSend(id, text); }
  baud = $state(115200);
  async copyStats() { if (!this.stats) return; const rows=[['channel','count','min','max','mean','rms','integral'].join('\t'),...orderedEntries(this.stats.channels, this.recording?.channels ?? []).map(([ch,s]) => [this.recording?.channels.find(c => c.name === ch)?.label ?? ch,s.count,s.min,s.max,s.mean,s.rms,s.integral].join('\t'))]; await navigator.clipboard.writeText(rows.join('\n')); toast.success('Selection stats copied'); }
  async refreshStats() {
    if (!this.recording || !this.range || this.range[0] === this.range[1]) return;
    const [t0, t1] = this.range, seq = ++this.statsSeq;
    try { const stats = await backend.getStats(t0, t1); if (seq === this.statsSeq) this.stats = stats; }
    catch (cause) { if (seq === this.statsSeq) this.report(cause); }
  }
  async refreshDetails() {
    if (!this.recording) return;
    const [t0, t1] = this.range ?? this.viewRange;
    try {
      const seq = ++this.detailsSeq, statsSeq = ++this.statsSeq;
      const uartFrom = this.uart.at(-1)?.t ?? 0;
      const [stats, tail, edges] = await Promise.all([
        backend.getStats(t0, t1), backend.getUart(uartFrom, this.recording.duration_s), backend.getEdges(0, this.recording.duration_s),
      ]);
      if (seq !== this.detailsSeq) return;
      if (statsSeq === this.statsSeq) this.stats = stats;
      this.uart = [...this.uart.filter(x => x.t < uartFrom), ...tail];
      this.edges = edges;
    } catch (cause) { this.report(cause); }
  }
}
export const app = new AppState();
