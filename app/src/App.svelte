<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  type Info = { name: string | null; fw: string | null; hw: string | null; uid: string | null };
  type Connection = { connected: boolean; info?: Info };
  type Settings = { main: boolean | null; mainvolt: number | null; maxcur: number | null; mainsrccurrmode: number | null; mainvolt_max: number | null; expvolt: number | null; gpo: (boolean | null)[]; fourwire: boolean | null };
  type Monitor = { mv: number | null; mc: number | null; mp: number | null; vb: number | null; vj: number | null; tp: number | null; gpi: number | null; fourwire: boolean | null };
  type Alert = { id: number; name: string };
  type AlertEntry = Alert & { key: number; time: string };
  let connection = $state<Connection>({ connected: false });
  let settings = $state<Settings | null>(null);
  let monitor = $state<Monitor>({ mv: null, mc: null, mp: null, vb: null, vj: null, tp: null, gpi: null, fourwire: null });
  let voltageInput = $state('');
  let currentInput = $state('');
  let busy = $state(false);
  let error = $state('');
  let alerts = $state<AlertEntry[]>([]);
  let toasts = $state<AlertEntry[]>([]);
  let nextKey = 0;
  const connected = $derived(connection.connected);
  const disabled = $derived(!connected || busy);
  function confirmed(next: Settings) {
    settings = next;
    voltageInput = next.mainvolt == null ? '' : next.mainvolt.toFixed(2);
    currentInput = next.maxcur == null ? '' : (next.maxcur * 1000).toFixed(1);
  }
  function reset() {
    settings = null;
    monitor = { mv: null, mc: null, mp: null, vb: null, vj: null, tp: null, gpi: null, fourwire: null };
    voltageInput = ''; currentInput = '';
  }
  onMount(() => {
    let alive = true;
    let unlisteners: (() => void)[] = [];
    (async () => {
      try {
        const handlers = [
          await listen<Connection>('connection', e => { connection = e.payload; if (!e.payload.connected) reset(); }),
          await listen<Settings>('settings', e => confirmed(e.payload)),
          await listen<Monitor>('monitor', e => { monitor = e.payload; }),
          await listen<Alert>('alert', e => {
            const entry = { ...e.payload, key: ++nextKey, time: new Date().toLocaleTimeString() };
            alerts = [entry, ...alerts].slice(0, 50);
            toasts = [entry, ...toasts];
            window.setTimeout(() => { toasts = toasts.filter(a => a.key !== entry.key); }, 8000);
          })
        ];
        if (!alive) { handlers.forEach(fn => fn()); return; }
        unlisteners = handlers;
        const state = await invoke<{ connection: Connection; settings: Settings | null }>('get_state');
        connection = state.connection;
        if (state.settings) confirmed(state.settings);
      } catch (cause) { error = String(cause); }
    })();
    return () => { alive = false; unlisteners.forEach(fn => fn()); };
  });
  async function apply(command: string, args: Record<string, unknown>) {
    if (disabled) return;
    busy = true; error = '';
    try { confirmed(await invoke<Settings>(command, args)); }
    catch (cause) { error = String(cause); if (settings) confirmed(settings); }
    finally { busy = false; }
  }
  function commitVoltage() {
    if (disabled || settings?.mainvolt == null || voltageInput === settings.mainvolt.toFixed(2)) return;
    const volts = Number(voltageInput);
    if (!voltageInput.trim() || !Number.isFinite(volts)) { error = 'Enter a valid voltage.'; confirmed(settings); return; }
    void apply('set_voltage', { volts });
  }
  function commitCurrent() {
    if (disabled || settings?.maxcur == null || currentInput === (settings.maxcur * 1000).toFixed(1)) return;
    const milliamps = Number(currentInput);
    if (!currentInput.trim() || !Number.isFinite(milliamps)) { error = 'Enter a valid current limit.'; confirmed(settings); return; }
    void apply('set_current_limit', { amps: milliamps / 1000 });
  }
  function enter(event: KeyboardEvent, commit: () => void) { if (event.key === 'Enter') { commit(); (event.target as HTMLInputElement).blur(); } }
  function scaled(value: number | null, units: [string, number][], digits = 4) {
    if (value == null) return { number: '—', unit: units.at(-1)![0] };
    const abs = Math.abs(value);
    const [unit, factor] = units.find(([, factor]) => abs >= factor) ?? units.at(-1)!;
    const formatted = (value / factor).toPrecision(digits);
    return { number: formatted.includes('.') ? formatted.replace(/\.?0+$/, '') : formatted, unit };
  }
  const current = $derived(scaled(monitor.mc, [['A', 1], ['mA', 1e-3], ['µA', 1e-6], ['nA', 1e-9]]));
  const power = $derived(scaled(monitor.mp, [['W', 1], ['mW', 1e-3]], 4));
</script>

<svelte:head><title>OpenOtii · Arc panel</title></svelte:head>
<div class="shell">
  <header>
    <div class="brand"><span class="eyebrow">OPENOTII / ARC</span><h1>Instrument panel</h1></div>
    <div class="device"><span class:online={connected} class="status-dot"></span><div><strong>{connected ? connection.info?.name || 'Otii Arc' : 'Waiting for Arc…'}</strong>{#if connected}<small>FW {connection.info?.fw || '—'} · HW {connection.info?.hw || '—'} · UID {connection.info?.uid || '—'}</small>{:else}<small>Connect an Arc to begin</small>{/if}</div></div>
  </header>
  {#if error}<div class="error" role="alert">{error}<button onclick={() => error = ''} aria-label="Dismiss error">×</button></div>{/if}
  <main>
    <section class="card psu"><div class="card-heading"><div><span class="eyebrow">01 / POWER SUPPLY</span><h2>Main output</h2></div><span class:active={settings?.main} class="output-state">{settings?.main ? 'OUTPUT ON' : 'OUTPUT OFF'}</span></div>
      <div class="readouts">
        <div class="readout"><span>VOLTAGE</span><div><strong>{monitor.mv == null ? '—' : monitor.mv.toFixed(4)}</strong><em>V</em></div></div>
        <div class="readout"><span>CURRENT</span><div><strong>{current.number}</strong><em>{current.unit}</em></div></div>
        <div class="readout"><span>POWER</span><div><strong>{power.number}</strong><em>{power.unit}</em></div></div>
      </div>
      <div class="psu-controls"><button class:enabled={settings?.main} class="output-button" disabled={disabled} onclick={() => void apply('set_output', { on: !settings?.main })}>{settings?.main ? '● Output enabled' : '○ Enable output'}</button>
        <label>Voltage setpoint <span>V · max {settings?.mainvolt_max?.toFixed(2) ?? '—'}</span><input type="number" min="0" max={settings?.mainvolt_max ?? undefined} step="0.01" bind:value={voltageInput} disabled={disabled} onblur={commitVoltage} onkeydown={e => enter(e, commitVoltage)} /></label>
        <label>Current limit <span>mA</span><input type="number" min="1" max="3000" step="0.1" bind:value={currentInput} disabled={disabled} onblur={commitCurrent} onkeydown={e => enter(e, commitCurrent)} /></label>
      </div>
      <div class="mode-row"><label for="mode">Current mode</label><select id="mode" disabled={disabled} value={settings?.mainsrccurrmode === 1 ? 'limit' : 'cutoff'} onchange={e => void apply('set_current_mode', { mode: e.currentTarget.value })}><option value="cutoff">Cutoff</option><option value="limit">Limit · constant current</option></select><p>{settings?.mainsrccurrmode === 1 ? 'Limits current by reducing output voltage.' : 'Switches output off when the limit is exceeded.'}</p></div>
    </section>
    <div class="side"><section class="card"><div class="card-heading"><div><span class="eyebrow">02 / TELEMETRY</span><h2>Device status</h2></div></div><dl class="status-list"><div><dt>VBUS</dt><dd>{monitor.vb == null ? '—' : `${monitor.vb.toFixed(3)} V`}</dd></div><div><dt>DC jack</dt><dd>{monitor.vj == null ? '—' : `${monitor.vj.toFixed(3)} V`}</dd></div><div><dt>Temperature</dt><dd>{monitor.tp == null ? '—' : `${monitor.tp.toFixed(1)} °C`}</dd></div><div><dt>4-wire</dt><dd>{monitor.fourwire ?? settings?.fourwire == null ? '—' : (monitor.fourwire ?? settings?.fourwire) ? 'On' : 'Off'}</dd></div><div><dt>Expansion voltage</dt><dd>{settings?.expvolt == null ? '—' : `${settings.expvolt.toFixed(2)} V`}</dd></div></dl></section>
    <section class="card"><div class="card-heading"><div><span class="eyebrow">03 / DIGITAL IO</span><h2>GPIO</h2></div></div><div class="gpio-group"><span class="group-label">OUTPUTS</span><div class="pins">{#each [1, 2, 3] as n}<button class:on={settings?.gpo[n-1]} disabled={disabled} onclick={() => void apply('set_gpo', { n, on: !settings?.gpo[n-1] })}>GPO{n}<span>{settings?.gpo[n-1] ? 'HIGH' : 'LOW'}</span></button>{/each}</div></div><div class="gpio-group"><span class="group-label">INPUTS</span><div class="pins inputs">{#each [1, 2, 3, 4, 5, 6] as n}<div class:high={monitor.gpi != null && !!(monitor.gpi & (1 << (n-1)))}><span class="pin-dot"></span>GPI{n}</div>{/each}</div></div></section>
    <section class="card alerts"><div class="card-heading"><div><span class="eyebrow">04 / EVENTS</span><h2>Alerts <span class="count">{alerts.length}</span></h2></div>{#if alerts.length}<button class="text-button" onclick={() => alerts = []}>Clear log</button>{/if}</div><div class="alert-log">{#if alerts.length}{#each alerts as alert (alert.key)}<div class:critical={alert.id === 6} class="alert-row"><time>{alert.time}</time><span>{alert.name}</span><small>#{alert.id}</small></div>{/each}{:else}<p class="empty">No alerts this session.</p>{/if}</div></section></div>
  </main>
</div>
<div class="toasts" aria-live="assertive">{#each toasts as toast (toast.key)}<div class:critical={toast.id === 6} class="toast"><div><strong>{toast.id === 6 ? 'OVERCURRENT' : 'DEVICE ALERT'}</strong><span>{toast.name}</span></div><button onclick={() => toasts = toasts.filter(a => a.key !== toast.key)} aria-label="Dismiss alert">×</button></div>{/each}</div>
