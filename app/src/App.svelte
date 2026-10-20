<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { toast } from 'svelte-sonner';
  import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '$lib/components/ui/dialog';
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
    if ('__TAURI_INTERNALS__' in window) {
      void getCurrentWindow().onCloseRequested(async event => {
        if (!app.unsaved || app.closingApproved) return;
        event.preventDefault();
        if (await app.confirmDiscard()) { app.closingApproved = true; await getCurrentWindow().close(); }
      }).then(off => { const prior=cleanup; cleanup=() => { prior?.(); off(); }; });
    }
    return () => { active = false; cleanup?.(); };
  });
  function shortcut(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    const editing = !!target.closest('input, textarea, select, [contenteditable="true"]');
    const key = event.key.toLowerCase();
    if (event.repeat) return;
    if (event.ctrlKey) {
      if (key === 'o') { event.preventDefault(); void app.openRecording(); }
      else if (key === 's') { event.preventDefault(); void app.saveRecording(); }
      else if (key === 'q') { event.preventDefault(); void app.quit(); }
      else if (key === 'j') { event.preventDefault(); app.drawerOpen = !app.drawerOpen; }
      else if (key === 'e' && app.connection.connected) { event.preventDefault(); void app.command('set_output',{on: !app.settings?.main}); }
      else if (key === 'a' && !editing) { event.preventDefault(); app.selectAll(); }
      else if (key === 'c' && event.shiftKey && !editing) { event.preventDefault(); void app.copyStats(); }
      return;
    }
    if (editing || event.altKey || event.metaKey || app.dialog) return;
    if (key === 'escape') app.clearSelection();
    else if (key === '?' ) app.showShortcuts();
    else if (key === 'r' && app.connection.connected) { event.preventDefault(); void app.toggleRecording(); }
    else if (key === 'f' && app.recording) app.zoomFit();
    else if ((key === '+' || key === '=') && app.recording) app.zoom(.5);
    else if (key === '-' && app.recording) app.zoom(2);
    else if (event.code === 'Space' && app.connection.connected && !target.closest('button')) {
      event.preventDefault(); void app.command('set_output', { on: !app.settings?.main }).then(ok => { if (ok) toast.info(app.settings?.main ? 'Output enabled' : 'Output disabled'); });
    }
  }

</script>
<svelte:head><title>{app.fileName ? `${app.fileName} · ` : ''}OpenOtii</title></svelte:head>
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
    <Dialog open={app.dialog !== null} onOpenChange={open => { if (!open) app.resolveDialog(false); }}>
      <DialogContent showCloseButton={app.dialog !== 'discard'}>
        <DialogHeader><DialogTitle>{app.dialog === 'discard' ? 'Discard unsaved recording?' : app.dialog === 'export' ? 'Export recording' : app.dialog === 'device' ? 'Device info' : app.dialog === 'about' ? 'About OpenOtii' : 'Keyboard shortcuts'}</DialogTitle>
        <DialogDescription>{app.dialog === 'discard' ? 'The current recording has unsaved changes.' : app.dialog === 'export' ? 'Choose the portion to export.' : ''}</DialogDescription></DialogHeader>
        {#if app.dialog === 'discard'}<DialogFooter><Button variant="outline" onclick={() => app.resolveDialog(false)}>Cancel</Button><Button onclick={() => app.resolveDialog(true)}>Discard</Button></DialogFooter>
        {:else if app.dialog === 'export'}<DialogFooter><Button variant="outline" onclick={() => { app.dialog=null; void app.exportRecording(app.exportFormat); }}>Whole recording</Button><Button onclick={() => { app.dialog=null; void app.exportRecording(app.exportFormat,true); }}>Selection</Button></DialogFooter>
        {:else if app.dialog === 'device'}<dl class="info-list"><dt>Name</dt><dd>{app.connection.info?.name ?? '—'}</dd><dt>Firmware</dt><dd>{app.connection.info?.fw ?? '—'}</dd><dt>Hardware</dt><dd>{app.connection.info?.hw ?? '—'}</dd><dt>UID</dt><dd>{app.connection.info?.uid ?? '—'}</dd><dt>Protocol</dt><dd>{app.connection.info?.proto ?? '—'}</dd></dl>
        {:else if app.dialog === 'about'}<p>OpenOtii 0.1.0</p>
        {:else if app.dialog === 'shortcuts'}<dl class="info-list">{#each [['Open recording','Ctrl+O'],['Save recording','Ctrl+S'],['Quit','Ctrl+Q'],['Select all','Ctrl+A'],['Clear selection','Esc'],['Copy selection stats','Ctrl+Shift+C'],['Zoom to fit','F'],['Zoom in','+ / ='],['Zoom out','−'],['Toggle bottom panel','Ctrl+J'],['Output on/off','Ctrl+E / Space'],['Start/stop recording','R'],['Keyboard shortcuts','?']] as [label, key]}<dt>{label}</dt><dd><kbd>{key}</kbd></dd>{/each}</dl>{/if}
      </DialogContent>
    </Dialog>
    <Toaster position="bottom-right" offset="calc(24% + 16px)" richColors/>
  </div>
</TooltipProvider>
