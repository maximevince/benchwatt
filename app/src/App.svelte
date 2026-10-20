<script lang="ts">
  import { onMount } from 'svelte';
  import { toast } from 'svelte-sonner';
  import { Toaster } from '$lib/components/ui/sonner';
  import { TooltipProvider } from '$lib/components/ui/tooltip';
  import { app } from '$lib/stores.svelte';
  import TopBar from '$lib/components/TopBar.svelte';
  import SupplyPanel from '$lib/components/SupplyPanel.svelte';
  import ChannelList from '$lib/components/ChannelList.svelte';
  import ExpansionPanel from '$lib/components/ExpansionPanel.svelte';
  import StatusPanel from '$lib/components/StatusPanel.svelte';
  import LiveChart from '$lib/components/LiveChart.svelte';
  import BottomDrawer from '$lib/components/BottomDrawer.svelte';
  import { PaneGroup, Pane, Handle } from '$lib/components/ui/resizable';
  let cleanup: (() => void) | undefined;
  const toggleDrawer = () => app.drawerOpen = !app.drawerOpen;
  onMount(() => {
    let active = true;
    void app.start().then(off => { if (active) cleanup = off; else off(); });
    return () => { active = false; cleanup?.(); };
  });
  function shortcut(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (event.repeat || target.closest('input, textarea, select, button, [contenteditable="true"]') || !app.connection.connected) return;
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
        <aside class="sidebar"><SupplyPanel/><ChannelList/><ExpansionPanel/><StatusPanel/></aside>
      </Pane>
      <Handle withHandle/>
      <Pane defaultSize={75} minSize={55} class="main-pane">
        {#if app.drawerOpen}
          <PaneGroup direction="vertical" class="main-panes">
            <Pane defaultSize={76} minSize={40}><LiveChart/></Pane>
            <Handle withHandle/>
            <Pane defaultSize={24} minSize={14} maxSize={45} collapsible collapsedSize={6} onCollapse={() => app.drawerOpen = false}>
              <BottomDrawer onToggle={toggleDrawer}/>
            </Pane>
          </PaneGroup>
        {:else}
          <div class="main-panes-collapsed"><LiveChart/><BottomDrawer onToggle={toggleDrawer}/></div>
        {/if}
      </Pane>
    </PaneGroup>
    <Toaster position="top-right" richColors/>
  </div>
</TooltipProvider>
