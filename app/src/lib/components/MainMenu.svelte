<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Menubar, MenubarMenu, MenubarTrigger, MenubarContent, MenubarItem, MenubarSeparator, MenubarShortcut, MenubarSub, MenubarSubTrigger, MenubarSubContent, MenubarCheckboxItem, MenubarRadioGroup, MenubarRadioItem } from '$lib/components/ui/menubar';
  import DeviceItems from './DeviceItems.svelte';
  const outputs = $derived(app.selected?.desc.outputs ?? []);
  const outputOn = (index: number) => app.selected?.state?.outputs[index]?.enabled ?? false;
</script>
<Menubar class="main-menu">
  <MenubarMenu><MenubarTrigger>File</MenubarTrigger><MenubarContent>
    <MenubarItem onclick={() => void app.openRecording()}>Open recording…<MenubarShortcut>Ctrl+O</MenubarShortcut></MenubarItem>
    <MenubarSeparator/>
    <MenubarItem disabled={!app.tab} onclick={() => void app.saveRecording()}>Save<MenubarShortcut>Ctrl+S</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.tab} onclick={() => void app.saveRecording(true)}>Save as…<MenubarShortcut>Ctrl+Shift+S</MenubarShortcut></MenubarItem>
    <MenubarSub><MenubarSubTrigger disabled={!app.tab}>Export</MenubarSubTrigger><MenubarSubContent><MenubarItem onclick={() => void app.requestExport('csv')}>CSV…</MenubarItem><MenubarItem onclick={() => void app.requestExport('sr')}>sigrok session (.sr)…</MenubarItem></MenubarSubContent></MenubarSub>
    <MenubarSeparator/>
    <MenubarItem disabled={!app.tab} onclick={() => void app.closeTab()}>Close recording<MenubarShortcut>Ctrl+W</MenubarShortcut></MenubarItem>
    <MenubarItem onclick={() => void app.quit()}>Quit<MenubarShortcut>Ctrl+Q</MenubarShortcut></MenubarItem>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>Edit</MenubarTrigger><MenubarContent>
    <MenubarItem disabled={!app.tab} onclick={() => app.selectAll()}>Select all<MenubarShortcut>Ctrl+A</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.range} onclick={() => app.clearSelection()}>Clear selection<MenubarShortcut>Esc</MenubarShortcut></MenubarItem>
    <MenubarSeparator/>
    <MenubarItem disabled={!app.range || !app.stats} onclick={() => void app.copyStats()}>Copy selection stats<MenubarShortcut>Ctrl+Shift+C</MenubarShortcut></MenubarItem>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>View</MenubarTrigger><MenubarContent>
    <MenubarItem disabled={!app.tab} onclick={() => app.zoomFit()}>Zoom to fit<MenubarShortcut>F</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.tab} onclick={() => app.zoom(.5)}>Zoom in<MenubarShortcut>+</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.tab} onclick={() => app.zoom(2)}>Zoom out<MenubarShortcut>−</MenubarShortcut></MenubarItem>
    <MenubarCheckboxItem disabled={!app.tab} checked={!!app.tab && app.follow} onclick={() => app.follow = !app.follow}>Follow while recording</MenubarCheckboxItem>
    <MenubarSub><MenubarSubTrigger disabled={!app.hiddenLanes.length}>Show hidden lane</MenubarSubTrigger><MenubarSubContent>{#each app.hiddenLanes as ch}<MenubarItem onclick={() => app.hiddenLanes=app.hiddenLanes.filter(lane => lane !== ch)}>{app.lanes.find(lane => lane.key === ch)?.label ?? ch}</MenubarItem>{/each}<MenubarSeparator/><MenubarItem onclick={() => app.hiddenLanes=[]}>All</MenubarItem></MenubarSubContent></MenubarSub>
    <MenubarSeparator/>
    <MenubarItem disabled={app.tabs.length < 2} onclick={() => app.cycleTab(1)}>Next recording<MenubarShortcut>Ctrl+Tab</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={app.tabs.length < 2} onclick={() => app.cycleTab(-1)}>Previous recording<MenubarShortcut>Ctrl+Shift+Tab</MenubarShortcut></MenubarItem>
    <MenubarSeparator/>
    <MenubarCheckboxItem checked={app.drawerOpen} onclick={() => app.drawerOpen = !app.drawerOpen}>Bottom panel<MenubarShortcut>Ctrl+J</MenubarShortcut></MenubarCheckboxItem>
    <MenubarSub><MenubarSubTrigger>Theme</MenubarSubTrigger><MenubarSubContent><MenubarRadioGroup value={app.theme}>{#each ['system','light','dark'] as theme}<MenubarRadioItem value={theme} onclick={() => app.setTheme(theme as 'system' | 'light' | 'dark')}>{theme[0].toUpperCase()+theme.slice(1)}</MenubarRadioItem>{/each}</MenubarRadioGroup></MenubarSubContent></MenubarSub>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>Device</MenubarTrigger><MenubarContent>
    <DeviceItems/>
    <MenubarSeparator/>
    <MenubarItem disabled={!app.selected && !app.live} onclick={() => void app.toggleRecording()}>{app.live ? 'Stop recording' : 'New recording'}<MenubarShortcut>R</MenubarShortcut></MenubarItem>
    {#if outputs.length === 1}<MenubarItem disabled={app.disabled} onclick={() => void app.toggleOutput()}>Switch output {outputOn(0) ? 'off' : 'on'}<MenubarShortcut>Space</MenubarShortcut></MenubarItem>
    {:else if outputs.length}<MenubarSub><MenubarSubTrigger disabled={app.disabled}>Outputs</MenubarSubTrigger><MenubarSubContent>{#each outputs as output, index (output.n)}<MenubarCheckboxItem checked={outputOn(index)} onclick={() => void app.command('set_output', { out: output.n, on: !outputOn(index) })}>{output.label}</MenubarCheckboxItem>{/each}</MenubarSubContent></MenubarSub>{/if}
    {#if app.selected?.desc.uart}<MenubarCheckboxItem checked={app.uartOpen} onclick={() => void app.toggleUart()}>UART open</MenubarCheckboxItem>{/if}
    <MenubarSeparator/>
    <MenubarItem disabled={!app.selected} onclick={() => app.dialog='device'}>Device info…</MenubarItem>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>Help</MenubarTrigger><MenubarContent>
    <MenubarItem onclick={() => app.showShortcuts()}>Keyboard shortcuts<MenubarShortcut>?</MenubarShortcut></MenubarItem>
    <MenubarItem onclick={() => app.dialog='about'}>About Benchwatt</MenubarItem>
  </MenubarContent></MenubarMenu>
</Menubar>
