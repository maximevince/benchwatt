<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  // Digital outputs (GPO) and the bits of the device's digital input channel.
  const desc = $derived(app.selected?.desc);
  const outputs = $derived(Array.from({ length: desc?.gpo ?? 0 }, (_, i) => i + 1));
  const inputs = $derived(Array.from({ length: desc?.channels.find(ch => ch.kind === 'Digital')?.bits ?? 0 }, (_, i) => i + 1));
  const gpo = (n: number) => app.selected?.state?.gpo[n-1] ?? null;
  const level = (on: boolean | null) => on == null ? '—' : on ? 'HIGH' : 'LOW';
</script>
{#if outputs.length || inputs.length}<section class="sidebar-section" aria-label="Digital I/O"><div class="section-heading"><h2>Digital I/O</h2></div>
  {#if outputs.length}<div class="subheading">OUTPUTS</div>
  <div class="gpo-list">{#each outputs as n}<Button size="sm" variant="outline" class={gpo(n) ? 'pin-on' : ''} disabled={app.disabled} onclick={() => void app.command('set_gpo', { n, on: !gpo(n) })}>GPO {n}<span>{level(gpo(n))}</span></Button>{/each}</div>{/if}
  {#if inputs.length}<div class="subheading" class:gpi-title={outputs.length}>INPUTS</div>
  <div class="gpi-list">{#each inputs as n}<div class="gpi-item"><i class:high={app.monitor?.inputs != null && !!(app.monitor.inputs & (1 << (n-1)))}></i><span>GPI {n}</span></div>{/each}</div>{/if}
</section>{/if}
