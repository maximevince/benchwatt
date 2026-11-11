<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { MenubarItem, MenubarLabel, MenubarRadioGroup, MenubarRadioItem, MenubarSeparator } from '$lib/components/ui/menubar';
  // The choice of device, as menu items: used by the Device menu and by the device button in the top bar.
</script>
{#if app.found.length}
  <MenubarRadioGroup value={app.wanted ?? ''}>{#each app.found as device (device.key)}
    <MenubarRadioItem value={device.key} closeOnSelect disabled={!!app.live && device.key !== app.wanted} onclick={() => void app.selectDevice(device.key)}>
      <span class="device-item"><span>{app.selected?.desc.id === device.key ? app.selected.desc.model : device.name}</span><small>{app.selected?.desc.id === device.key && app.selected.desc.serial ? app.selected.desc.serial : device.port}</small></span>
    </MenubarRadioItem>
  {/each}</MenubarRadioGroup>
{:else}
  <MenubarLabel class="device-none">No supported instrument is plugged in</MenubarLabel>
{/if}
<MenubarSeparator/>
<MenubarItem disabled={!app.wanted || !!app.live} onclick={() => void app.selectDevice(null)}>Disconnect</MenubarItem>
