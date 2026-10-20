<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { channel } from '$lib/channels';
  const channelIds = ['mc', 'mv', 'mp', 'ac', 'av', 'vb', 'tp', 'gpi', 'rx'];
  function choose(key: string, on: boolean) { app.recordChannels = on ? [...app.recordChannels, key] : app.recordChannels.filter(x => x !== key); }
</script>
<section class="sidebar-section" aria-label="Channels"><div class="section-heading"><h2>Channels</h2><span class="section-meta">RECORD</span></div><div class="channel-list">{#each channelIds as id}<label class="channel-row" title={id}><input type="checkbox" checked={app.recordChannels.includes(id)} disabled={app.recording?.active} onchange={e => choose(id, e.currentTarget.checked)}/><i class="swatch" data-channel={channel(id).color}></i><span>{channel(id).label}</span></label>{/each}</div></section>
