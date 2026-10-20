<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
</script>
<section class="sidebar-section" aria-label="Expansion"><div class="section-heading"><h2>Expansion</h2></div><div class="subheading">DIGITAL OUTPUTS</div><div class="gpo-list">{#each [1,2,3] as n}<Button size="sm" variant="outline" class={app.settings?.gpo[n-1] ? 'pin-on' : ''} disabled={app.disabled} onclick={() => void app.command('set_gpo', { n, on: !app.settings?.gpo[n-1] })}>GPO {n}<span>{app.settings?.gpo[n-1] == null ? '—' : app.settings.gpo[n-1] ? 'HIGH' : 'LOW'}</span></Button>{/each}</div><div class="subheading gpi-title">DIGITAL INPUTS</div><div class="gpi-list">{#each [1,2,3,4,5,6] as n}<div class="gpi-item"><i class:high={app.monitor.gpi != null && !!(app.monitor.gpi & (1 << (n-1)))}></i><span>{n}</span></div>{/each}</div></section>
