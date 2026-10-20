<script lang="ts">
  import { onMount } from 'svelte';
  import uPlot from 'uplot';
  import 'uplot/dist/uPlot.min.css';
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import { Pause, Play, Trash2 } from 'lucide-svelte';
  import { formatReading } from '$lib/format';
  let host: HTMLDivElement;
  let chart: uPlot | undefined;
  let size = $state({ width: 700, height: 350 });
  let cursor = $state<number | null>(null);
  let latest = $derived(cursor == null ? app.samples.at(-1) : app.samples.reduce((best, sample) => Math.abs(sample.time - cursor!) < Math.abs((best?.time ?? Infinity) - cursor!) ? sample : best, app.samples[0]));
  const colors = { dark: { current: '#fbbf24', voltage: '#38bdf8', power: '#a78bfa', grid: '#27272a', text: '#a1a1aa' }, light: { current: '#d97706', voltage: '#0284c7', power: '#7c3aed', grid: '#e4e4e7', text: '#71717a' } };
  function render() {
    if (!host || size.width < 100 || size.height < 100) return;
    chart?.destroy();
    const c = colors[app.theme];
    chart = new uPlot({
      width: size.width, height: size.height, cursor: { drag: { x: false, y: false } },
      scales: { x: { time: true }, current: { auto: true }, voltage: { auto: true }, power: { auto: true } },
      axes: [
        { stroke: c.text, grid: { stroke: c.grid }, ticks: { stroke: c.grid }, size: 34, values: (_u, values) => values.map(v => new Date(v * 1000).toLocaleTimeString('en-GB', { minute: '2-digit', second: '2-digit' })) },
        { scale: 'current', stroke: c.current, grid: { stroke: c.grid }, ticks: { stroke: c.grid }, size: 68, values: (_u, values) => values.map(v => { const reading = formatReading(v, 'current'); return `${reading.number.trim()} ${reading.unit}`; }) },
        { scale: 'voltage', side: 1, stroke: c.voltage, grid: { show: false }, ticks: { stroke: c.grid }, size: 58, values: (_u, values) => values.map(v => v.toFixed(2)) },
      ],
      series: [
        {},
        { label: 'Current', scale: 'current', stroke: c.current, width: 1.8, points: { show: false } },
        { label: 'Voltage', scale: 'voltage', stroke: c.voltage, width: 1.8, points: { show: false } },
        { label: 'Power', scale: 'power', stroke: c.power, width: 1.6, points: { show: false } },
      ],
      hooks: { setCursor: [u => { const index = u.cursor.idx; cursor = index == null ? null : (u.data[0][index] ?? null); }] },
      legend: { show: false },
    }, [[], [], [], []], host);
    updateData();
  }
  function updateData() {
    if (!chart) return;
    const samples = app.samples;
    const data: uPlot.AlignedData = [samples.map(s => s.time), samples.map(s => s.mc), samples.map(s => s.mv), samples.map(s => app.showPower ? s.mp : null)];
    chart.setData(data);
    const now = Date.now() / 1000;
    chart.setScale('x', { min: now - 60, max: now });
    const currentMax = Math.max(0.05, ...samples.map(s => Math.abs(s.mc ?? 0)));
    const voltageMax = Math.max(4, ...samples.map(s => Math.abs(s.mv ?? 0)));
    chart.setScale('current', { min: 0, max: Math.ceil(currentMax * 100) / 100 });
    chart.setScale('voltage', { min: 0, max: Math.ceil(voltageMax) });
  }
  onMount(() => {
    const observer = new ResizeObserver(entries => { const rect = entries[0].contentRect; size = { width: Math.floor(rect.width), height: Math.floor(rect.height) }; });
    observer.observe(host);
    return () => { observer.disconnect(); chart?.destroy(); chart = undefined; };
  });
  $effect(() => { app.theme; size; render(); });
  $effect(() => { app.samples; app.showPower; updateData(); });
</script>
<section class="chart-panel" aria-label="Live chart"><div class="chart-header"><div><div class="eyebrow">LIVE MONITOR</div><h1>Output over time</h1></div><div class="chart-actions"><Badge variant="outline" class={app.connection.connected ? 'live-badge' : 'live-badge offline'}><i></i> {app.connection.connected ? 'LIVE · 60 S' : 'OFFLINE'}</Badge><Button variant="outline" size="sm" disabled={!app.connection.connected} onclick={() => app.paused = !app.paused} aria-label={app.paused ? 'Resume chart' : 'Pause chart'}>{#if app.paused}<Play size={14}/>{:else}<Pause size={14}/>{/if}{app.paused ? 'Resume' : 'Pause'}</Button><Button variant="outline" size="icon" disabled={!app.connection.connected} onclick={() => app.clearChart()} aria-label="Clear chart"><Trash2 size={14}/></Button></div></div><div class="chart-legend"><div data-channel="current"><i class="swatch" data-channel="current"></i><span>Current</span><strong>{formatReading(latest?.mc ?? null, 'current').number.trim()} <small>{formatReading(latest?.mc ?? null, 'current').unit}</small></strong></div><div data-channel="voltage"><i class="swatch" data-channel="voltage"></i><span>Voltage</span><strong>{formatReading(latest?.mv ?? null, 'voltage').number.trim()} <small>{formatReading(latest?.mv ?? null, 'voltage').unit}</small></strong></div>{#if app.showPower}<div data-channel="power"><i class="swatch" data-channel="power"></i><span>Power</span><strong>{formatReading(latest?.mp ?? null, 'power').number.trim()} <small>{formatReading(latest?.mp ?? null, 'power').unit}</small></strong></div>{/if}</div><div class="chart-stage"><div class="chart-canvas" bind:this={host}></div>{#if !app.connection.connected}<div class="waiting-overlay"><div class="waiting-icon">◌</div><strong>Waiting for Arc…</strong><p>Connect an Otii Arc to see live measurements.</p><small>On Linux, check that the udev rule is installed.</small></div>{/if}</div><div class="chart-footer"><span>← 60 seconds</span><span>{!app.connection.connected ? 'OFFLINE' : app.paused ? 'PAUSED' : 'LIVE'} · 10 Hz monitor</span><span>Now →</span></div></section>
