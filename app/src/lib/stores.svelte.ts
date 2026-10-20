import { toast } from 'svelte-sonner';
import { backend, type Alert, type Command, type Connection, type Monitor, type Settings, type RecordingInfo, type AllStats, type UartChunk, type Edge } from './backend';
import { describeAlert } from './alerts';

export type AlertEntry = Alert & { key: number; time: string };
const emptyMonitor = (): Monitor => ({ mv: null, mc: null, mp: null, vb: null, vj: null, tp: null, gpi: null, fourwire: null });
class AppState {
  connection = $state<Connection>({ connected: false });
  settings = $state<Settings | null>(null);
  monitor = $state<Monitor>(emptyMonitor());
  alerts = $state<AlertEntry[]>([]);
  busy = $state(false);
  error = $state('');
  theme = $state<'dark' | 'light'>('dark');
  showPower = $state(false);
  drawerOpen = $state(true);
  recording = $state<RecordingInfo | null>(null);
  recordChannels = $state<string[]>(['mc', 'mv', 'mp']);
  range = $state<[number, number] | null>(null);
  viewRange = $state<[number, number]>([0, 10]);
  stats = $state<AllStats | null>(null);
  uart = $state<UartChunk[]>([]);
  edges = $state<Edge[]>([]);
  uartOpen = $state(false);
  private key = 0;
  get disabled() { return !this.connection.connected || this.busy; }
  initTheme() {
    const saved = localStorage.getItem('openotii-theme');
    this.theme = saved === 'light' || saved === 'dark' ? saved : matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    this.applyTheme();
  }
  toggleTheme() { this.theme = this.theme === 'dark' ? 'light' : 'dark'; localStorage.setItem('openotii-theme', this.theme); this.applyTheme(); }
  private applyTheme() { document.documentElement.classList.toggle('dark', this.theme === 'dark'); }
  async start(): Promise<() => void> {
    this.initTheme();
    const off = await Promise.all([
      backend.listen('connection', value => { this.connection = value; if (!value.connected) { this.settings = null; this.monitor = emptyMonitor(); this.uartOpen = false; } }),
      backend.listen('settings', value => { this.settings = value; }),
      backend.listen('monitor', value => { this.monitor = value; }),
      backend.listen('alert', value => {
        const entry = { ...value, key: ++this.key, time: new Date().toLocaleTimeString() };
        this.alerts = [entry, ...this.alerts].slice(0, 100);
        const details = describeAlert(value.id);
        const message = `${details.title}: ${details.description}`;
        if (details.severity === 'warning') toast.warning(message);
        else toast.error(message);
      }),
      backend.listen('recording', value => { this.recording = value; }),
    ]);
    try {
      const snapshot = await backend.getState();
      this.connection = snapshot.connection;
      this.settings = snapshot.settings;
      this.recording = await backend.recordingInfo();
    } catch (cause) { this.report(cause); }
    return () => off.forEach(fn => fn());
  }
  async command(name: Command, args: Record<string, unknown>) {
    if (this.disabled) return false;
    this.busy = true; this.error = '';
    try { this.settings = await backend.command(name, args); return true; }
    catch (cause) { this.report(cause); return false; }
    finally { this.busy = false; }
  }
  report(cause: unknown) { this.error = String(cause); toast.error(this.error); }
  async toggleRecording() {
    try {
      if (this.recording?.active) { this.recording = await backend.stopRecording(); return; }
      if (this.recording && !window.confirm('Replace the current recording? It cannot be recovered until export is available.')) return;
      this.recording = await backend.startRecording(this.recordChannels);
      this.range = null;
      this.viewRange = [0, 10];
      this.stats = null;
      this.uart = [];
      this.edges = [];
    } catch (cause) { this.report(cause); }
  }
  async refreshDetails() {
    if (!this.recording) return;
    const [t0, t1] = this.range ?? this.viewRange;
    try {
      [this.stats, this.uart, this.edges] = await Promise.all([
        backend.getStats(t0, t1), backend.getUart(0, this.recording.duration_s), backend.getEdges(0, this.recording.duration_s),
      ]);
    } catch (cause) { this.report(cause); }
  }
}
export const app = new AppState();
