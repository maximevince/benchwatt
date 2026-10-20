<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import { Tooltip, TooltipContent, TooltipTrigger } from '$lib/components/ui/tooltip';
  import { Moon, Sun, CircleHelp, Activity } from 'lucide-svelte';
  let uid = $derived(app.connection.info?.uid ? app.connection.info.uid.slice(0, 4) + '…' : '—');
</script>
<header class="topbar"><div class="brand"><Activity size={18} strokeWidth={2.3}/><strong>OpenOtii</strong><span class="brand-divider"></span><span class="brand-subtitle">ARC MONITOR</span></div><div class="topbar-right"><Badge variant="outline" class="device-badge"><i class:connected={app.connection.connected}></i><span>{app.connection.connected ? (app.connection.info?.name || 'Arc') : 'Disconnected'}</span>{#if app.connection.connected}<span class="device-detail">· fw {app.connection.info?.fw ?? '—'} · {uid}</span>{/if}</Badge><Tooltip><TooltipTrigger><Button variant="ghost" size="icon" onclick={() => app.toggleTheme()} aria-label="Toggle theme">{#if app.theme === 'dark'}<Sun size={17}/>{:else}<Moon size={17}/>{/if}</Button></TooltipTrigger><TooltipContent>Toggle theme</TooltipContent></Tooltip><Tooltip><TooltipTrigger><Button variant="ghost" size="icon" aria-label="Keyboard shortcuts"><CircleHelp size={17}/></Button></TooltipTrigger><TooltipContent>Space: toggle output · R: recording in Phase 3</TooltipContent></Tooltip></div></header>
