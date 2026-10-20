<script lang="ts">
  import { onMount } from 'svelte';
  import { toast } from 'svelte-sonner';
  import { Toaster } from '$lib/components/ui/sonner';
  import { TooltipProvider } from '$lib/components/ui/tooltip';
  import { app } from '$lib/stores.svelte';
  import { Button } from '$lib/components/ui/button';
  import TopBar from '$lib/components/TopBar.svelte';
  import SupplyPanel from '$lib/components/SupplyPanel.svelte';
  import ChannelList from '$lib/components/ChannelList.svelte';
  import ExpansionPanel from '$lib/components/ExpansionPanel.svelte';
  import StatusPanel from '$lib/components/StatusPanel.svelte';
  import RecordingPlot from '$lib/components/RecordingPlot.svelte';
  import BottomDrawer from '$lib/components/BottomDrawer.svelte';
  import { PaneGroup, Pane, Handle } from '$lib/components/ui/resizable';
  import { ScrollArea } from '$lib/components/ui/scroll-area';
  let cleanup: (() => void) | undefined;
  const toggleDrawer = () => app.drawerOpen = !app.drawerOpen;
  onMount(() => {
    let active = true;
    void app.start().then(off => { if (active) cleanup = off; else off(); });
    return () => { active = false; cleanup?.(); };
  });
  function shortcut(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (event.repeat || target.closest('input, textarea, select, [contenteditable="true"]') || !app.connection.connected) return;
    if (event.code === 'KeyR') { event.preventDefault(); void app.toggleRecording(); return; }
    if (target.closest('button')) return;
    if (event.code === 'Space') {
      event.preventDefault();
      void app.command('set_output', { on: !app.settings?.main }).then(ok => {
        if (ok) toast.info(app.settings?.main ? 'Output enabled' : 'Output disabled');
      });
    }
  }
</script>
<svelte:head><title>OpenOtii · Arc monitor</title></svelte:head>
<svelte:window onkeydown={shortcut}/>
<TooltipProvider>
  <div class="app-shell">
    <TopBar/>
    <PaneGroup direction="horizontal" class="workspace">
      <Pane defaultSize={25} minSize={20} maxSize={36} class="sidebar-pane">
        <ScrollArea class="sidebar-scroll"><aside class="sidebar"><SupplyPanel/><ChannelList/><ExpansionPanel/><StatusPanel/></aside></ScrollArea>
      </Pane>
      <Handle withHandle/>
      <Pane defaultSize={75} minSize={55} class="main-pane">
        {#if app.drawerOpen}
          <PaneGroup direction="vertical" class="main-panes">
            <Pane defaultSize={76} minSize={40}><div class="plot-container">{#if app.recording}<RecordingPlot/>{:else}<div class="plot-empty"><p>No recording yet</p><Button onclick={() => app.toggleRecording()} disabled={!app.connection.connected}>Record <kbd>R</kbd></Button><small>Live values are in the sidebar; choose channels under Channels.</small></div>{/if}</div></Pane>
            <Handle withHandle/>
            <Pane defaultSize={24} minSize={14} maxSize={45} collapsible collapsedSize={6} onCollapse={() => app.drawerOpen = false}>
              <BottomDrawer onToggle={toggleDrawer}/>
            </Pane>
          </PaneGroup>
        {:else}
          <div class="main-panes-collapsed"><div class="plot-container">{#if app.recording}<RecordingPlot/>{:else}<div class="plot-empty"><p>No recording yet</p><Button onclick={() => app.toggleRecording()} disabled={!app.connection.connected}>Record <kbd>R</kbd></Button><small>Live values are in the sidebar; choose channels under Channels.</small></div>{/if}</div><BottomDrawer onToggle={toggleDrawer}/></div>
        {/if}
      </Pane>
    </PaneGroup>
    <Toaster position="bottom-right" offset="calc(24% + 16px)" richColors/>
  </div>
</TooltipProvider>
