<script lang="ts">
  import { app } from '$lib/stores.svelte';
  const channels = [{ key: 'mc', label: 'Current', color: 'current' }, { key: 'mv', label: 'Voltage', color: 'voltage' }, { key: 'mp', label: 'Power', color: 'power' }, { key: 'vb', label: 'VBUS', color: 'aux' }, { key: 'gpi', label: 'GPI', color: 'digital' }];
</script>
<section class="sidebar-section" aria-label="Channels"><div class="section-heading"><h2>Channels</h2><span class="section-meta">LIVE</span></div><div class="channel-list">{#each channels as channel}<div class="channel-row"><i class="swatch" data-channel={channel.color}></i><span>{channel.label}</span><code>{channel.key}</code>{#if channel.key === 'mp'}<button class:muted={!app.showPower} onclick={() => app.showPower = !app.showPower} aria-label="Toggle power trace">{app.showPower ? 'Shown' : 'Hidden'}</button>{/if}</div>{/each}</div></section>
