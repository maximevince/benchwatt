<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  // Shown in the sidebar while no device is open: what is plugged in, to pick from.
  const wanted = $derived(app.found.find(f => f.key === app.wanted));
</script>
<section class="sidebar-section device-chooser">
  {#if app.connecting && wanted}
    <h2>Connecting…</h2><p>{wanted.name} on {wanted.port}</p>
  {:else if app.deviceError && wanted}
    <h2>Could not open the {wanted.name}</h2><p class="device-error">{app.deviceError}</p>
    <p>Another program may be using it. Benchwatt keeps trying.</p>
    <Button variant="outline" size="sm" onclick={() => void app.selectDevice(null)}>Stop trying</Button>
  {:else if app.wanted && !wanted}
    <h2>Device unplugged</h2><p>It is picked up again when you plug it back in.</p>
  {:else if app.found.length}
    <h2>Choose a device</h2><p>Benchwatt works with one instrument at a time.</p>
  {:else}
    <h2>No device found</h2><p>Plug in an Otii Arc, a GW Instek GPP supply, a Nordic PPK2 or a Joulescope JS220.</p>
  {/if}
  {#if !app.connecting}<div class="device-choices">{#each app.found.filter(f => f.key !== app.wanted || !app.deviceError) as device (device.key)}
    <button onclick={() => void app.selectDevice(device.key)}><strong>{device.name}</strong><small>{device.port}</small></button>
  {/each}</div>{/if}
</section>
