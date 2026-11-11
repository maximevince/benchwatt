<script lang="ts">
  import type { AllStats } from '$lib/backend';
  import type { ChannelDesc } from '$lib/backend';
  import { orderedEntries, channelColors, formatValue, outputTag } from '$lib/channels';
  import ChannelName from './ChannelName.svelte';
  import { formatReading } from '$lib/format';
  import { app } from '$lib/stores.svelte';
  import { Copy, ChevronDown, ChevronUp, X } from 'lucide-svelte';

  let { range, viewRange, width, stats }: { range: [number, number]; viewRange: [number, number]; width: number; stats: AllStats | null } = $props();
  let collapsed = $state(sessionStorage.getItem('selection-card-collapsed') === 'true');
  const cardWidth = $derived(Math.min(collapsed ? 330 : 400, Math.max(0, width - 8)));
  const start = $derived((range[0] - viewRange[0]) / (viewRange[1] - viewRange[0]) * width);
  const end = $derived((range[1] - viewRange[0]) / (viewRange[1] - viewRange[0]) * width);
  const left = $derived(Math.max(4, Math.min(width - cardWidth - 4, end + 8 + cardWidth <= width ? end + 8 : start - cardWidth - 8)));
  const channels = $derived(app.recording?.channels ?? []);
  const colors = $derived(channelColors(channels, app.recordingOutputs));
  const desc = (id: string): ChannelDesc => channels.find(ch => ch.name === id) ?? { id: -1, name: id, label: id, unit: '', rate_hz: 0, kind: 'Analog', quantity: 'Other', group: '', bits: 0, extra: false, set: false };
  const analog = $derived(orderedEntries(stats?.channels ?? {}, channels).filter(([id]) => desc(id).kind === 'Analog'));
  const has = (quantity: ChannelDesc['quantity']) => analog.some(([id]) => desc(id).quantity === quantity);

  function time(value: number) { return value.toFixed(4) + ' s'; }
  function duration(value: number) { return value < .001 ? (value * 1e6).toFixed(1) + ' µs' : value < 1 ? (value * 1e3).toFixed(2) + ' ms' : value < 120 ? value.toFixed(3) + ' s' : (value / 60).toFixed(2) + ' min'; }
  function reading(value: number, id: string) { return formatValue(value, desc(id)); }
  function power(value: number) { const result = formatReading(value, 'power'); return result.number.trim() + ' ' + result.unit; }
  function integrated(value: number, unit: 'Ah' | 'Wh') { const milli = value / 3.6; return Math.abs(milli) < 1 ? (milli * 1000).toFixed(3) + ' µ' + unit : milli.toFixed(3) + ' m' + unit; }
  function integral(value: number, id: string) { const q = desc(id).quantity; return q === 'Current' ? integrated(value, 'Ah') : q === 'Power' ? integrated(value, 'Wh') : ''; }
  function integralTitle(value: number, id: string) { const q = desc(id).quantity; return q === 'Current' ? (value * 1000).toFixed(3) + ' mC' : q === 'Power' ? (value * 1000).toFixed(3) + ' mJ' : ''; }
  function toggle() { collapsed = !collapsed; sessionStorage.setItem('selection-card-collapsed', String(collapsed)); }
</script>

<aside class="selection-card" class:collapsed style:left={left + 'px'} style:width={cardWidth + 'px'} aria-label="Selection measurements" onpointerdown={event => event.stopPropagation()}>
  <div class="selection-card-heading"><strong>{duration(range[1] - range[0])}</strong><time>{time(range[0])} – {time(range[1])}</time><div class="selection-card-actions"><button title="Copy selection stats as TSV" aria-label="Copy selection stats" onclick={() => void app.copyStats()}><Copy size={13}/></button><button title={collapsed ? 'Expand measurements' : 'Collapse measurements'} aria-label={collapsed ? 'Expand measurements' : 'Collapse measurements'} onclick={toggle}>{#if collapsed}<ChevronDown size={13}/>{:else}<ChevronUp size={13}/>{/if}</button><button title="Clear selection" aria-label="Clear selection" onclick={() => app.clearSelection()}><X size={13}/></button></div></div>
  {#if !collapsed}<div class="selection-card-body">
    {#if stats}<div class="selection-card-grid"><span></span><span>min</span><span>avg</span><span>max</span><span title="Integral">∫</span>
      {#each analog as [id, values]}<div class="selection-card-label" title={id}><ChannelName label={desc(id).label} color={colors[id] ?? 'aux'} output={outputTag(app.recordingOutputs, desc(id))}/></div><code>{reading(values.min, id)}</code><code>{reading(values.mean, id)}</code><code>{reading(values.max, id)}</code><code title={integralTitle(values.integral, id)}>{integral(values.integral, id)}</code>{/each}
    </div>
    {#if stats.energy}<div class="selection-card-footer">{#if !has('Power') && has('Current') && has('Voltage')}<span>Energy</span><code title={(stats.energy.joules * 1000).toFixed(3) + ' mJ'}>{integrated(stats.energy.joules, 'Wh')}</code>{/if}<span>Average power</span><code>{power(stats.energy.average_power)}</code></div>{/if}
    {:else}<p class="selection-card-loading">Calculating…</p>{/if}
  </div>{/if}
</aside>
