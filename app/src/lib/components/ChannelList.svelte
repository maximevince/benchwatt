<script lang="ts">
  import type { ChannelDesc } from '$lib/backend';
  import { app } from '$lib/stores.svelte';
  import { channelColors, rxChannel } from '$lib/channels';
  import { ChevronDown, ChevronRight } from 'lucide-svelte';
  const channels = $derived(app.selected?.desc.channels ?? []);
  const colors = $derived(channelColors(channels));
  const rx = $derived(rxChannel(channels)?.name);
  const extra = $derived(channels.filter(ch => ch.extra));
  let more = $state(false);
  function choose(name: string, on: boolean) { const chosen = on ? [...app.recordChannels, name] : app.recordChannels.filter(x => x !== name); app.recordChannels = channels.map(ch => ch.name).filter(x => chosen.includes(x)); }
</script>
{#snippet row(ch: ChannelDesc)}{@const forced = ch.name === rx && app.uartOpen}<label class="channel-row" title={forced ? 'Always recorded while the UART is open' : ch.name}><input type="checkbox" checked={app.recordChannels.includes(ch.name) || forced} disabled={app.recording?.active || forced} onchange={e => choose(ch.name, e.currentTarget.checked)}/><i class="swatch" data-channel={colors[ch.name]}></i><span>{ch.label}</span></label>{/snippet}
<section class="sidebar-section" aria-label="Channels"><div class="section-heading"><h2>Channels</h2><span class="section-meta">RECORD</span></div><div class="channel-list">{#each channels.filter(ch => !ch.extra) as ch (ch.name)}{@render row(ch)}{/each}{#if extra.length}<button class="channel-more" aria-expanded={more} onclick={() => more = !more}>{#if more}<ChevronDown size={13}/>{:else}<ChevronRight size={13}/>{/if}More<span>{extra.length}</span></button>{#if more}{#each extra as ch (ch.name)}{@render row(ch)}{/each}{/if}{/if}</div></section>
