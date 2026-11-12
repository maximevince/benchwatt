<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Menubar, MenubarMenu, MenubarTrigger, MenubarContent } from '$lib/components/ui/menubar';
  import { Tooltip, TooltipContent, TooltipTrigger } from '$lib/components/ui/tooltip';
  import { Moon, Sun, CircleHelp, Activity, ChevronDown } from 'lucide-svelte';
  import MainMenu from './MainMenu.svelte';
  import DeviceItems from './DeviceItems.svelte';
  const wanted = $derived(app.found.find(f => f.key === app.wanted));
  const clock = (s: number) => `${Math.floor(s / 60).toString().padStart(2, '0')}:${Math.floor(s % 60).toString().padStart(2, '0')}`;
</script>
<header class="topbar"><div class="brand"><Activity size={18} strokeWidth={2.3}/><strong>Benchwatt</strong><span class="brand-divider"></span><span class="brand-subtitle">POWER MONITOR</span></div><MainMenu/>
  <div class="topbar-right">
    <Menubar class="device-picker"><MenubarMenu><MenubarTrigger aria-label="Device">
      <i class="device-dot" class:connected={!!app.selected?.state} class:waiting={app.connecting} class:failed={!!app.deviceError}></i>
      {#if app.selected}<span>{app.selected.desc.model}</span><span class="device-detail">{app.selected.desc.serial}</span>{#if app.serial.open}<span class="device-plus">+</span><span class="device-serial">{app.serial.port?.split('/').pop()}</span>{/if}
      {:else if wanted}<span>{wanted.name}</span><span class="device-detail">{app.deviceError ? 'cannot open' : 'connecting…'}</span>
      {:else if app.wanted}<span class="device-detail">Unplugged</span>
      {:else}<span>{app.found.length ? 'Choose a device' : 'No device'}</span>{/if}
      <ChevronDown size={14}/>
    </MenubarTrigger><MenubarContent align="end"><DeviceItems/></MenubarContent></MenubarMenu></Menubar>
    <Button variant="outline" size="sm" disabled={!app.selected && !app.live} onclick={() => void app.toggleRecording()} aria-label={app.live ? 'Stop recording' : 'Start recording'}><span class:record-pulse={!!app.live} class="record-dot"></span>{app.live ? `Stop ${clock(app.live.info.duration_s)}` : 'Record'}</Button>
    <Tooltip><TooltipTrigger><Button variant="ghost" size="icon" onclick={() => app.toggleTheme()} aria-label="Toggle theme">{#if app.darkTheme}<Sun size={17}/>{:else}<Moon size={17}/>{/if}</Button></TooltipTrigger><TooltipContent>Toggle theme</TooltipContent></Tooltip>
    <Tooltip><TooltipTrigger><Button variant="ghost" size="icon" onclick={() => app.showShortcuts()} aria-label="Keyboard shortcuts"><CircleHelp size={17}/></Button></TooltipTrigger><TooltipContent>Keyboard shortcuts (?)</TooltipContent></Tooltip>
  </div>
</header>
