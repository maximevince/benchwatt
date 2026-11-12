<script lang="ts">
  import { app, DEVICE_UART } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Switch } from '$lib/components/ui/switch';
  // Where the console's text comes from: the device's own UART or a serial port of the PC, and how that port is set up.
  const OTHER = '\0other';
  let typing = $state(false), typed = $state(''), settingsOpen = $state(false), panel = $state<HTMLElement>();
  // The panel opens upwards from its button, fixed to the window so the short bottom panel cannot clip it.
  let place = $state('');
  function toggleSettings(event: MouseEvent) {
    const r = (event.currentTarget as HTMLElement).getBoundingClientRect();
    place = `left:${Math.max(8, Math.min(r.left, window.innerWidth - 300))}px;bottom:${window.innerHeight - r.top + 6}px`;
    settingsOpen = !settingsOpen;
  }
  const isPort = $derived(!!app.textSource && app.textSource !== DEVICE_UART);
  const s = $derived(app.portSettings);
  const framing = $derived(`${s.data_bits}${s.parity[0].toUpperCase()}${s.stop_bits}`);
  // Anything that is not the default shows on the button, so an unusual setting is never hidden.
  const summary = $derived([framing, s.flow === 'rtscts' ? 'RTS/CTS' : s.flow === 'xonxoff' ? 'XON/XOFF' : '', s.dtr ? '' : 'DTR off', s.rts || s.flow === 'rtscts' ? '' : 'RTS off'].filter(Boolean).join(' · '));
  const shortName = (path: string) => path.split('/').pop() ?? path;
  // A port that is chosen but not plugged in right now still needs its entry.
  const missing = $derived(isPort && !app.ports.some(p => p.path === app.textSource) ? app.textSource : null);
  function pick(value: string) {
    if (value === OTHER) { typing = true; typed = ''; return; }
    void app.chooseSource(value || null);
  }
  function useTyped() { const path = typed.trim(); typing = false; if (path) void app.chooseSource(path); }
  function set<K extends keyof typeof s>(key: K, value: (typeof s)[K]) { app.portSettings = { ...app.portSettings, [key]: value }; }
  function outside(event: MouseEvent) { if (settingsOpen && panel && !panel.contains(event.target as Node)) settingsOpen = false; }
</script>
<svelte:window onclick={outside} onkeydown={e => { if (e.key === 'Escape') settingsOpen = false; }}/>
<label>Port
  {#if typing}
    <!-- svelte-ignore a11y_autofocus -->
    <input class="serial-path" placeholder="/dev/ttyUSB0" aria-label="Serial port path" autofocus bind:value={typed} onkeydown={e => { if (e.key === 'Enter') useTyped(); else if (e.key === 'Escape') typing = false; }} onblur={useTyped}/>
  {:else}
    <select class="serial-port" aria-label="Source of text" value={app.textSource ?? ''} disabled={!app.selected} onpointerdown={() => void app.refreshPorts()} onfocus={() => void app.refreshPorts()} onchange={e => { pick(e.currentTarget.value); e.currentTarget.value = app.textSource ?? ''; }}>
      {#if !app.textSource}<option value="">Choose a port</option>{/if}
      {#if app.selected?.desc.uart}<optgroup label="This instrument"><option value={DEVICE_UART}>{app.selected.desc.model} UART</option></optgroup>{/if}
      <optgroup label="Serial ports on this PC">
        {#each app.ports as port (port.path)}<option value={port.path}>{port.name ? `${port.name} · ${shortName(port.path)}` : port.path}</option>{/each}
        {#if missing}<option value={missing}>{missing}</option>{/if}
        <option value={OTHER}>Other port…</option>
      </optgroup>
    </select>
  {/if}
</label>
{#if app.textSource === DEVICE_UART}
  <label>Baud <input type="number" min="1" max="3000000" bind:value={app.baud} disabled={app.textOpen}/></label>
{:else if isPort}
  <label>Baud <input type="number" min="1" max="4000000" value={s.baud} disabled={app.textOpen} onchange={e => set('baud', Math.max(1, Math.round(e.currentTarget.valueAsNumber) || 115200))}/></label>
  <span class="serial-settings" bind:this={panel}>
    <button class="serial-summary" class:active={settingsOpen} aria-expanded={settingsOpen} aria-label={`Port settings: ${summary}`} onclick={toggleSettings}>{summary} ▾</button>
    {#if settingsOpen}<div class="serial-panel" role="dialog" aria-label="Port settings" style={place}>
      <label>Data bits<select value={s.data_bits} disabled={app.textOpen} onchange={e => set('data_bits', Number(e.currentTarget.value))}>{#each [8, 7, 6, 5] as n}<option value={n}>{n}</option>{/each}</select></label>
      <label>Parity<select value={s.parity} disabled={app.textOpen} onchange={e => set('parity', e.currentTarget.value as typeof s.parity)}><option value="none">None</option><option value="even">Even</option><option value="odd">Odd</option></select></label>
      <label>Stop bits<select value={s.stop_bits} disabled={app.textOpen} onchange={e => set('stop_bits', Number(e.currentTarget.value))}><option value={1}>1</option><option value={2}>2</option></select></label>
      <label>Flow control<select value={s.flow} disabled={app.textOpen} onchange={e => set('flow', e.currentTarget.value as typeof s.flow)}><option value="none">None</option><option value="rtscts">RTS/CTS</option><option value="xonxoff">XON/XOFF</option></select></label>
      {#if app.textOpen}<p>Close the port to change these.</p>{/if}
      <hr/>
      <h3>Control lines</h3>
      <div class="serial-line"><span>DTR</span><Switch checked={s.dtr} onCheckedChange={on => void app.setLine('dtr', on)} aria-label="DTR"/><small>{s.dtr ? 'asserted' : 'released'}</small></div>
      <div class="serial-line" class:off={s.flow === 'rtscts'}><span>RTS</span><Switch checked={s.rts} disabled={s.flow === 'rtscts'} onCheckedChange={on => void app.setLine('rts', on)} aria-label="RTS"/><small>{s.flow === 'rtscts' ? 'driven by flow control' : s.rts ? 'asserted' : 'released'}</small></div>
      <div class="serial-line"><Button size="sm" variant="outline" disabled={!app.textOpen} onclick={() => void app.pulseDtr()}>Pulse DTR</Button><small>Resets boards that wire DTR to reset.</small></div>
      <p>Opening a port raises DTR and RTS for a moment, whatever is set here.</p>
    </div>{/if}
  </span>
{/if}
{#if app.textSource}<Button size="sm" variant="outline" disabled={!app.selected?.state} onclick={() => void app.toggleText()}>{app.textOpen ? 'Close' : 'Open'}</Button>{/if}
