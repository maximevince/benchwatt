import { toast } from 'svelte-sonner';
import { open, save } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { documentDir, join } from '@tauri-apps/api/path';
import { backend, type Alert, type Command, type Device, type Devices, type DeviceState, type FoundDevice, type Monitor, type OutputDesc, type PortInfo, type RecordingInfo, type SerialInfo, type SerialSettings, type AllStats, type UartChunk, type Edge } from './backend';
import { defaultSerialSettings } from './serial';
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

const LAST_DEVICE_KEY = 'benchwatt.device';
/** Remembered per kind of instrument: where its text came from last. */
const SOURCE_KEY = 'benchwatt.text.';
/** Remembered per serial port: how it was set up last. */
const PORT_KEY = 'benchwatt.port.';
function stored<T>(key: string): T | null {
  try { const text = localStorage.getItem(key); return text ? JSON.parse(text) as T : null; } catch { return null; }
}
function store(key: string, value: unknown) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* storage unavailable */ }
}
/** Marks the device's own UART as the source of text, as opposed to a port path. */
export const DEVICE_UART = 'device';
const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;

export type AlertEntry = Alert & { key: number; time: string };
type Range = [number, number];
/** One open recording and how it is being looked at. Each is a tab above the plot. */
export class Tab {
  info = $state<RecordingInfo>(null!);
  /** Where it was saved or opened from; empty until then. */
  path = $state('');
  unsaved = $state(false);
  /** The outputs of the device it was recorded from, for their names and colours. A file does not carry them yet. */
  outputs: OutputDesc[];
  /** When the capture started, for the title of a recording that has no file. */
  started: string;
  hiddenLanes = $state<string[]>([]);
  range = $state<Range | null>(null);
  viewRange = $state<Range>([0, 10]);
  stats = $state<AllStats | null>(null);
  uart = $state<UartChunk[]>([]);
  edges = $state<Edge[]>([]);
  marker = $state<number | null>(null);
  hoverMarker = $state<number | null>(null);
  focusedUartTime = $state<number | null>(null);
  follow = $state(true);
  detailsSeq = 0;
  statsSeq = 0;
  constructor(info: RecordingInfo, outputs: OutputDesc[] = [], path = '') {
    this.info = info; this.outputs = outputs; this.path = path;
    this.started = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
    if (!info.active) this.viewRange = [0, Math.max(info.duration_s, .001)];
  }
  get id() { return this.info.id; }
  get fileName() { return baseName(this.path); }
  /** The instrument it was made with, e.g. `GPP-4323`. */
  get source() { return this.info.source.model ?? this.info.source.kind ?? ''; }
  get title() { return this.fileName || `${this.source || 'Recording'} ${this.started}`; }
}
class AppState {
  /** Supported instruments that are plugged in. */
  found = $state<FoundDevice[]>([]);
  /** The key of the device the user chose; it is opened whenever it is plugged in. */
  wanted = $state<string | null>(null);
  /** The device the app works with, once it is open. Only one is at a time. */
  selected = $state<Device | null>(null);
  /** Why the chosen device could not be opened. */
  deviceError = $state<string | null>(null);
  connecting = $derived(!!this.wanted && !this.selected && !this.deviceError && this.found.some(f => f.key === this.wanted));
  /** Set once the user disconnects, so a device is not picked for them again. */
  private released = false;
  monitor = $state<Monitor | null>(null);
  /** Record channel choices per device id; a device without a choice records its defaults. */
  recordChoice = $state<Record<string, string[]>>({});
  alerts = $state<AlertEntry[]>([]);
  busy = $state(false);
  error = $state('');
  theme = $state<'system' | 'dark' | 'light'>('system');
  systemDark = $state(false);
  closingApproved = false;
  dialog = $state<'discard' | 'export' | 'shortcuts' | 'about' | 'device' | null>(null);
  /** What the discard dialog asks. */
  discardText = $state('');
  private dialogResolve?: (value: boolean) => void;
  drawerTab = $state('alerts');
  showPower = $state(false);
  drawerOpen = $state(true);
  tabs = $state<Tab[]>([]);
  tabId = $state<number | null>(null);
  /** The recording on screen. */
  tab = $derived(this.tabs.find(t => t.id === this.tabId) ?? null);
  /** The recording that is being captured, on screen or not. */
  live = $derived(this.tabs.find(t => t.info.active) ?? null);
  anyUnsaved = $derived(this.tabs.some(t => t.unsaved));
  // The state of the recording on screen, under the names the components use.
  get recording() { return this.tab?.info ?? null; }
  get fileName() { return this.tab?.fileName ?? ''; }
  get unsaved() { return this.tab?.unsaved ?? false; }
  get hiddenLanes() { return this.tab?.hiddenLanes ?? []; }
  set hiddenLanes(value) { if (this.tab) this.tab.hiddenLanes = value; }
  get range() { return this.tab?.range ?? null; }
  set range(value) { if (this.tab) this.tab.range = value; }
  get viewRange(): Range { return this.tab?.viewRange ?? [0, 10]; }
  set viewRange(value) { if (this.tab) this.tab.viewRange = value; }
  get stats() { return this.tab?.stats ?? null; }
  get uart() { return this.tab?.uart ?? []; }
  get edges() { return this.tab?.edges ?? []; }
  get marker() { return this.tab?.marker ?? null; }
  set marker(value) { if (this.tab) this.tab.marker = value; }
  get hoverMarker() { return this.tab?.hoverMarker ?? null; }
  set hoverMarker(value) { if (this.tab) this.tab.hoverMarker = value; }
  get focusedUartTime() { return this.tab?.focusedUartTime ?? null; }
  set focusedUartTime(value) { if (this.tab) this.tab.focusedUartTime = value; }
  get follow() { return this.tab?.follow ?? true; }
  set follow(value) { if (this.tab) this.tab.follow = value; }
  get recordChannels(): string[] { const d = this.selected; return d ? this.recordChoice[d.desc.id] ?? defaultRecordChannels(d.desc) : []; }
  set recordChannels(channels: string[]) { const d = this.selected; if (d) this.recordChoice = { ...this.recordChoice, [d.desc.id]: channels }; }
  /** Outputs of the device the recording on screen was made with (their colours and names tag the channels). */
  recordingOutputs = $derived(this.tab?.outputs ?? []);
  /** Plot lanes of the recording on screen, from its own channel list. */
  lanes = $derived(lanesFor(this.recording?.channels ?? [], this.recordingOutputs));
  get uartOpen() { return this.selected?.state?.uart_open ?? false; }
  /** The serial port linked to the device, as the backend has it. */
  serial = $state<SerialInfo>({ port: null, open: false, settings: defaultSerialSettings });
  ports = $state<PortInfo[]>([]);
  /** Where text comes from: the device's own UART, a serial port's path, or nothing chosen yet. A recording has one source. */
  textSource = $state<string | null>(null);
  /** How the chosen serial port is opened. */
  portSettings = $state<SerialSettings>({ ...defaultSerialSettings });
  get textOpen() { return this.textSource === DEVICE_UART ? this.uartOpen : this.serial.open && this.serial.port === this.textSource; }
  async refreshPorts() { try { this.ports = await backend.serialPorts(); } catch (cause) { this.report(cause); } }
  /** Picks the source of text, closing the one that was open. */
  async chooseSource(source: string | null) {
    if (source === this.textSource) return;
    if (this.textOpen) await this.toggleText();
    this.textSource = source;
    if (source && source !== DEVICE_UART) this.portSettings = { ...defaultSerialSettings, ...stored<SerialSettings>(PORT_KEY + source) };
    if (this.selected) store(SOURCE_KEY + this.selected.desc.kind, source);
  }
  /** Opens or closes the chosen source of text. */
  async toggleText() {
    const source = this.textSource, device = this.selected;
    if (!source || !device) return;
    if (source === DEVICE_UART) return this.toggleUart();
    try {
      if (this.serial.open) await backend.serialClose();
      else {
        store(PORT_KEY + source, this.portSettings);
        await backend.serialOpen(device.desc.id, source, $state.snapshot(this.portSettings));
        this.drawerTab = 'uart'; this.drawerOpen = true;
      }
    } catch (cause) { this.report(cause); }
  }
  async sendText(text: string) {
    if (this.textSource === DEVICE_UART) await this.uartSend(text); else await backend.serialSend(text);
  }
  /** Sets a control line of the serial port: now if it is open, and for the next time it is opened. */
  async setLine(line: 'dtr' | 'rts', on: boolean) {
    this.portSettings = { ...this.portSettings, [line]: on };
    if (this.textSource && this.textSource !== DEVICE_UART) store(PORT_KEY + this.textSource, this.portSettings);
    if (this.serial.open) try { await backend.serialLines({ [line]: on }); } catch (cause) { this.report(cause); }
  }
  async pulseDtr() { try { await backend.serialLines({ pulse: true }); } catch (cause) { this.report(cause); } }
  /** The source of text this device used last: its own UART if it has one and nothing else was chosen. */
  private restoreSource() {
    const device = this.selected;
    const saved = device ? stored<string>(SOURCE_KEY + device.desc.kind) : null;
    const source = this.serial.open ? this.serial.port : saved === DEVICE_UART && !device?.desc.uart ? null : saved ?? (device?.desc.uart ? DEVICE_UART : null);
    this.textSource = source;
    this.portSettings = source && source !== DEVICE_UART ? { ...defaultSerialSettings, ...(this.serial.open ? this.serial.settings : stored<SerialSettings>(PORT_KEY + source)) } : { ...defaultSerialSettings };
  }
  private key = 0;
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
      backend.listen('serial', value => { this.serial = value; }),
      backend.listen('state', value => this.setState(value.device, value.state)),
      backend.listen('monitor', value => { if (this.selected?.desc.id === value.device) this.monitor = value; }),
      backend.listen('alert', value => {
        const entry = { ...value, key: ++this.key, time: new Date().toLocaleTimeString() };
        this.alerts = [entry, ...this.alerts].slice(0, 100);
        const message = `${value.title}: ${value.detail}`;
        if (value.severity === 'warning') toast.warning(message);
        else toast.error(message);
      }),
      backend.listen('recording', value => {
        const tab = this.tabs.find(t => t.id === value.id);
        if (!tab) return;
        if (tab.info.active && value.duration_s > tab.info.duration_s) tab.unsaved = true;
        tab.info = value;
      }),
    ]);
    try {
      this.serial = await backend.serialInfo();
      this.setDevices(await backend.getDevices());
      void this.refreshPorts();
      // Recordings outlive a reload of the window.
      this.tabs = (await backend.recordings()).map(info => new Tab(info));
      this.tabId = this.tabs.at(-1)?.id ?? null;
    } catch (cause) { this.report(cause); }
    return () => { off.forEach(fn => fn()); media.removeEventListener('change', changed); };
  }
  private setDevices(devices: Devices) {
    this.found = devices.found; this.wanted = devices.wanted; this.deviceError = devices.error;
    const changed = devices.active?.desc.id !== this.selected?.desc.id;
    this.selected = devices.active;
    if (changed) { this.monitor = null; this.restoreSource(); }
    if (devices.wanted || this.released) return;
    // Nothing chosen yet: take the device used last time, or the only one there is.
    let last: string | null = null;
    try { last = localStorage.getItem(LAST_DEVICE_KEY); } catch { /* storage unavailable */ }
    const pick = devices.found.find(f => f.key === last) ?? devices.found.find(f => f.kind === last?.split('@')[0]) ?? (devices.found.length === 1 ? devices.found[0] : null);
    if (pick) void this.selectDevice(pick.key);
  }
  private setState(id: string, state: DeviceState) { if (this.selected?.desc.id === id) this.selected = { ...this.selected, state }; }
  /** Makes this the device the app works with; `null` lets go of the current one. */
  async selectDevice(key: string | null) {
    if (key === this.wanted && !this.deviceError) return;
    if (this.live) { toast.info('Stop the recording before changing the device'); return; }
    this.released = key === null;
    if (key) try { localStorage.setItem(LAST_DEVICE_KEY, key); } catch { /* storage unavailable */ }
    try { await backend.selectDevice(key); } catch (cause) { this.report(cause); }
  }
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
  /** Whether the one thing that powers the target is on: a supply's only output, or the
   *  `power` setting of a device that switches a supply line (the Joulescope). `null` when
   *  there is no such single switch. */
  get powerOn(): boolean | null {
    const device = this.selected;
    if (!device) return null;
    if (device.desc.outputs.length === 1) return device.state?.outputs[0]?.enabled ?? false;
    if (!device.desc.outputs.length && device.desc.settings.some(s => s.key === 'power' && s.type === 'bool' && !s.readonly)) return device.state?.settings.power === true;
    return null;
  }
  /** Toggles that single switch; with several outputs there is no default. */
  async toggleOutput() {
    const device = this.selected, was = this.powerOn;
    if (!device || was === null) return null;
    const output = device.desc.outputs[0];
    const done = output ? await this.command('set_output', { out: output.n, on: !was }) : await this.command('set_setting', { key: 'power', value: !was });
    return done ? !was : null;
  }
  /** Stops the running capture, or starts one in a new tab. */
  async toggleRecording() {
    try {
      if (this.live) { await backend.stopRecording(); return; }
      const device = this.selected;
      if (!device) return;
      // A device may only stream received UART bytes with its UART open, so recording them opens it.
      const rx = rxChannel(device.desc.channels);
      if (device.desc.uart && rx && this.recordChannels.includes(rx.name) && !this.uartOpen) await this.toggleUart();
      const tab = new Tab(await backend.startRecording(device.desc.id, this.recordChannels), device.desc.outputs);
      tab.unsaved = true;
      this.tabs.push(tab); this.tabId = tab.id;
    } catch (cause) { this.report(cause); }
  }
  showTab(tab: Tab) { this.tabId = tab.id; }
  /** Shows the tab `step` places to the right, wrapping around. */
  cycleTab(step: number) {
    const at = this.tabs.findIndex(t => t.id === this.tabId), n = this.tabs.length;
    if (n > 1) this.tabId = this.tabs[(at + step + n) % n].id;
  }
  async closeTab(tab: Tab | null = this.tab) {
    if (!tab) return;
    if (tab.info.active && !await this.confirm(`“${tab.title}” is still recording. Closing it stops the recording and discards it.`)) return;
    else if (!tab.info.active && tab.unsaved && !await this.confirm(`“${tab.title}” has not been saved.`)) return;
    try { await backend.closeRecording(tab.id); } catch (cause) { this.report(cause); return; }
    const at = this.tabs.indexOf(tab);
    this.tabs = this.tabs.filter(t => t !== tab);
    if (this.tabId === tab.id) this.tabId = (this.tabs[at] ?? this.tabs[at - 1])?.id ?? null;
  }
  private confirm(text: string): Promise<boolean> {
    this.discardText = text; this.dialog = 'discard';
    return new Promise(resolve => { this.dialogResolve = resolve; });
  }
  /** Asks before unsaved recordings are lost by leaving the app. */
  async confirmQuit(): Promise<boolean> {
    const unsaved = this.tabs.filter(t => t.unsaved);
    if (!unsaved.length) return true;
    return this.confirm(unsaved.length === 1 ? `“${unsaved[0].title}” has not been saved.` : `${unsaved.length} recordings have not been saved.`);
  }
  resolveDialog(value: boolean) { this.dialogResolve?.(value); this.dialogResolve = undefined; this.dialog = null; }
  private mock() { return !('__TAURI_INTERNALS__' in window); }
  /** Saves the recording on screen to its file; asks for one the first time or with `as`. */
  async saveRecording(as = false) {
    const tab = this.tab;
    if (!tab) return;
    try {
      let path = as ? '' : tab.path;
      if (!path) {
        const chosen = this.mock() ? 'mock.bwrec' : await save({ defaultPath: await inLastDir(tab.fileName || 'recording.bwrec'), filters: [{ name: 'Benchwatt recording', extensions: ['bwrec'] }] });
        if (!chosen) return;
        path = chosen; rememberDir(path);
      }
      const growing = tab.info.active;
      await backend.saveRecording(tab.id, path); tab.path = path; tab.unsaved = growing; toast.success(`Saved ${tab.fileName}`);
    } catch (cause) { this.report(cause); }
  }
  /** Opens a saved recording in a new tab. */
  async openRecording() {
    try { const path = this.mock() ? 'mock.bwrec' : await open({ multiple: false, defaultPath: await inLastDir(''), filters: [{ name: 'Benchwatt recording', extensions: ['bwrec'] }] });
      if (typeof path !== 'string') return; rememberDir(path);
      const tab = new Tab(await backend.loadRecording(path), [], path);
      this.tabs.push(tab); this.tabId = tab.id; await this.refreshDetails();
    } catch (cause) { this.report(cause); }
  }
  async exportRecording(extension: 'csv' | 'sr', selected = false) {
    const tab = this.tab;
    if (!tab) return;
    try { const path = this.mock() ? `mock.${extension}` : await save({ defaultPath: await inLastDir(`${tab.fileName.replace(/\.bwrec$/, '') || 'export'}.${extension}`), filters: [{ name: extension === 'csv' ? 'CSV' : 'sigrok session', extensions: [extension] }] });
      if (!path) return; rememberDir(path); await backend.exportRecording(tab.id, path, ...(selected && tab.range ? tab.range : [])); toast.success(`Exported ${baseName(path)}`);
    } catch (cause) { this.report(cause); }
  }
  async requestExport(extension: 'csv' | 'sr') {
    if (this.range) { this.exportFormat = extension; this.dialog = 'export'; } else await this.exportRecording(extension);
  }
  exportFormat = $state<'csv' | 'sr'>('csv');
  async quit() { if (!await this.confirmQuit()) return; if (this.mock()) toast.info('Quit is available in the desktop app'); else { this.closingApproved = true; await getCurrentWindow().destroy(); } }
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
      const state = this.selected?.desc.id === id ? this.selected.state : null; if (state) this.setState(id, { ...state, uart_open: open });
      if (open) { this.drawerTab='uart'; this.drawerOpen=true; }
    } catch (cause) { this.report(cause); }
  }
  async uartSend(text: string) { const id = this.selected?.desc.id; if (!id) throw new Error('No device'); await backend.uartSend(id, text); }
  baud = $state(115200);
  async copyStats() { if (!this.stats) return; const rows=[['channel','count','min','max','mean','rms','integral'].join('\t'),...orderedEntries(this.stats.channels, this.recording?.channels ?? []).map(([ch,s]) => [this.recording?.channels.find(c => c.name === ch)?.label ?? ch,s.count,s.min,s.max,s.mean,s.rms,s.integral].join('\t'))]; await navigator.clipboard.writeText(rows.join('\n')); toast.success('Selection stats copied'); }
  /** Statistics of the selection in the recording on screen. */
  async refreshStats() {
    const tab = this.tab;
    if (!tab || !tab.range || tab.range[0] === tab.range[1]) return;
    const [t0, t1] = tab.range, seq = ++tab.statsSeq;
    try { const stats = await backend.getStats(tab.id, t0, t1); if (seq === tab.statsSeq) tab.stats = stats; }
    catch (cause) { if (seq === tab.statsSeq && this.tabs.includes(tab)) this.report(cause); }
  }
  /** Statistics, UART bytes and digital edges of the recording on screen. */
  async refreshDetails() {
    const tab = this.tab;
    if (!tab) return;
    const [t0, t1] = tab.range ?? tab.viewRange;
    try {
      const seq = ++tab.detailsSeq, statsSeq = ++tab.statsSeq;
      const uartFrom = tab.uart.at(-1)?.t ?? 0;
      const [stats, tail, edges] = await Promise.all([
        backend.getStats(tab.id, t0, t1), backend.getUart(tab.id, uartFrom, tab.info.duration_s), backend.getEdges(tab.id, 0, tab.info.duration_s),
      ]);
      if (seq !== tab.detailsSeq) return;
      if (statsSeq === tab.statsSeq) tab.stats = stats;
      tab.uart = [...tab.uart.filter(x => x.t < uartFrom), ...tail];
      tab.edges = edges;
    } catch (cause) { if (this.tabs.includes(tab)) this.report(cause); }
  }
}
export const app = new AppState();
