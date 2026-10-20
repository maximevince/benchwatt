<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  import SetpointInput from './SetpointInput.svelte';
  // The Arc's expansion port has GPO1–2, GPI1–2 and a switchable 5 V pin; the other gpi bits aren't wired.
  const gpo = (n: number) => app.settings?.gpo[n-1] ?? null;
  const level = (on: boolean | null, high = 'HIGH', low = 'LOW') => on == null ? '—' : on ? high : low;
</script>
<section class="sidebar-section" aria-label="Expansion"><div class="section-heading"><h2>Expansion</h2></div>
  <SetpointInput label="Digital voltage" value={app.settings?.expvolt ?? null} suffix="V" hint="1.2–5 V" min={1.2} max={5} step={0.1} precision={2} disabled={app.disabled} apply={v => app.command('set_exp_voltage', { volts: v })} />
  <div class="subheading gpo-title">OUTPUTS</div>
  <div class="gpo-list">
    {#each [1,2] as n}<Button size="sm" variant="outline" class={gpo(n) ? 'pin-on' : ''} disabled={app.disabled} onclick={() => void app.command('set_gpo', { n, on: !gpo(n) })}>GPO {n}<span>{level(gpo(n))}</span></Button>{/each}
    <Button size="sm" variant="outline" class={app.settings?.five_volt ? 'pin-on' : ''} disabled={app.disabled} title="5 V supply pin on the expansion port" onclick={() => void app.command('set_5v', { on: !app.settings?.five_volt })}>5 V<span>{level(app.settings?.five_volt ?? null, 'ON', 'OFF')}</span></Button>
  </div>
  <div class="subheading gpi-title">INPUTS</div>
  <div class="gpi-list">{#each [1,2] as n}<div class="gpi-item"><i class:high={app.monitor.gpi != null && !!(app.monitor.gpi & (1 << (n-1)))}></i><span>GPI {n}</span></div>{/each}</div>
</section>
