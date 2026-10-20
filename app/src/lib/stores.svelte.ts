import { toast } from 'svelte-sonner';
import { backend, type Alert, type Command, type Connection, type Monitor, type Settings } from './backend';

export type AlertEntry = Alert & { key: number; time: string };
export type Sample = { time: number; mc: number | null; mv: number | null; mp: number | null };
const emptyMonitor = (): Monitor => ({ mv: null, mc: null, mp: null, vb: null, vj: null, tp: null, gpi: null, fourwire: null });
class AppState {
  connection = $state<Connection>({ connected: false });
  settings = $state<Settings | null>(null);
  monitor = $state<Monitor>(emptyMonitor());
  alerts = $state<AlertEntry[]>([]);
  samples = $state<Sample[]>([]);
  busy = $state(false);
  error = $state('');
  theme = $state<'dark' | 'light'>('dark');
  showPower = $state(false);
  paused = $state(false);
  drawerOpen = $state(true);
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
      backend.listen('connection', value => { this.connection = value; if (!value.connected) { this.settings = null; this.monitor = emptyMonitor(); this.samples = []; } }),
      backend.listen('settings', value => { this.settings = value; }),
      backend.listen('monitor', value => {
        this.monitor = value;
        if (!this.paused) {
          const time = Date.now() / 1000;
          this.samples = [...this.samples.filter(s => s.time >= time - 60), { time, mc: value.mc, mv: value.mv, mp: value.mp }];
        }
      }),
      backend.listen('alert', value => {
        const entry = { ...value, key: ++this.key, time: new Date().toLocaleTimeString() };
        this.alerts = [entry, ...this.alerts].slice(0, 100);
        if (/overcurr|overvolt|overtemp/.test(value.name)) toast.error(value.name.replaceAll('-', ' '));
        else toast.warning(value.name.replaceAll('-', ' '));
      }),
    ]);
    try {
      const snapshot = await backend.getState();
      this.connection = snapshot.connection;
      this.settings = snapshot.settings;
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
  clearChart() { this.samples = []; }
}
export const app = new AppState();
