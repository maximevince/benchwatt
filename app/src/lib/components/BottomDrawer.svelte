<script lang="ts">
  import { app } from '$lib/stores.svelte';
  let { onToggle }: { onToggle: () => void } = $props();
  import { Tabs, TabsList, TabsTrigger, TabsContent } from '$lib/components/ui/tabs';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { ScrollArea } from '$lib/components/ui/scroll-area';
  import { ChevronDown, ChevronUp } from 'lucide-svelte';
  import AlertsLog from './AlertsLog.svelte';
  import UartConsole from './UartConsole.svelte';
  import { formatReading } from '$lib/format';
  import { channelColors, formatValue, orderedEntries } from '$lib/channels';
  function metric(value: number, kind: 'current' | 'voltage' | 'power') { const v = formatReading(value, kind); return v.number.trim() + ' ' + v.unit; }
  const channels = $derived(app.recording?.channels ?? []);
  const colors = $derived(channelColors(channels));
  const entries = $derived(orderedEntries(app.stats?.channels ?? {}, channels));
  const label = (ch: string) => channels.find(c => c.name === ch)?.label ?? ch;
  function statMetric(value: number, ch: string) { const desc = channels.find(c => c.name === ch); return desc ? formatValue(value, desc) : String(value); }
</script>
<section class="drawer" class:closed={!app.drawerOpen} aria-label="Bottom drawer"><Tabs bind:value={app.drawerTab} class="drawer-tabs"><div class="drawer-header"><TabsList><TabsTrigger value="alerts">Alerts <Badge variant="secondary">{app.alerts.length}</Badge></TabsTrigger><TabsTrigger value="uart">UART</TabsTrigger><TabsTrigger value="stats">Selection stats</TabsTrigger></TabsList><Button variant="ghost" size="icon" onclick={onToggle} aria-label={app.drawerOpen ? 'Collapse drawer' : 'Expand drawer'}>{#if app.drawerOpen}<ChevronDown size={16}/>{:else}<ChevronUp size={16}/>{/if}</Button></div>{#if app.drawerOpen}<div class="drawer-content"><TabsContent value="alerts"><ScrollArea class="drawer-scroll"><AlertsLog /></ScrollArea></TabsContent><TabsContent value="uart"><UartConsole/></TabsContent><TabsContent value="stats">{#if app.stats}<div class="selection-stats"><div class="stats-table" style:grid-template-columns={'80px repeat('+Object.keys(app.stats.channels).length+', minmax(115px, 1fr))'}><div></div>{#each entries as [ch]}<div class="stats-channel"><i class="swatch" data-channel={colors[ch] ?? 'aux'}></i>{label(ch)}</div>{/each}{#each ['mean','min','max','rms','p-p'] as row}<strong>{row}</strong>{#each entries as [ch, stat]}<code>{statMetric(row === 'p-p' ? stat.max-stat.min : stat[row as 'mean' | 'min' | 'max' | 'rms'], ch)}</code>{/each}{/each}</div><div class="stats-summary"><div><strong>Duration</strong><code>{((app.range ?? app.viewRange)[1]-(app.range ?? app.viewRange)[0]).toFixed(4)} s</code></div>{#if app.stats.charge_mah != null}<div><strong>Charge</strong><code>{Math.abs(app.stats.charge_mah) < 1 ? (app.stats.charge_mah * 1000).toFixed(3) + ' µAh' : app.stats.charge_mah.toFixed(3) + ' mAh'}</code></div>{/if}{#if app.stats.energy}<div><strong>Energy</strong><code>{Math.abs(app.stats.energy.mwh) < 1 ? (app.stats.energy.mwh * 1000).toFixed(3) + ' µWh' : app.stats.energy.mwh.toFixed(3) + ' mWh'}</code></div><div><strong>Average power</strong><code>{metric(app.stats.energy.average_power, 'power')}</code></div>{/if}</div></div>{:else}<p class="empty-state">Select a range in a recording to view statistics.</p>{/if}</TabsContent></div>{/if}</Tabs></section>
