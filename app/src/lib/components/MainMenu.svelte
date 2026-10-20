<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Menubar, MenubarMenu, MenubarTrigger, MenubarContent, MenubarItem, MenubarSeparator, MenubarShortcut, MenubarSub, MenubarSubTrigger, MenubarSubContent, MenubarCheckboxItem, MenubarRadioGroup, MenubarRadioItem } from '$lib/components/ui/menubar';
</script>
<Menubar class="main-menu">
  <MenubarMenu><MenubarTrigger>File</MenubarTrigger><MenubarContent>
    <MenubarItem onclick={() => void app.openRecording()}>Open recording…<MenubarShortcut>Ctrl+O</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.recording} onclick={() => void app.saveRecording()}>Save recording…<MenubarShortcut>Ctrl+S</MenubarShortcut></MenubarItem>
    <MenubarSub><MenubarSubTrigger disabled={!app.recording}>Export</MenubarSubTrigger><MenubarSubContent><MenubarItem onclick={() => void app.requestExport('csv')}>CSV…</MenubarItem><MenubarItem onclick={() => void app.requestExport('sr')}>sigrok session (.sr)…</MenubarItem></MenubarSubContent></MenubarSub>
    <MenubarSeparator/><MenubarItem onclick={() => void app.quit()}>Quit<MenubarShortcut>Ctrl+Q</MenubarShortcut></MenubarItem>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>Edit</MenubarTrigger><MenubarContent>
    <MenubarItem disabled={!app.recording} onclick={() => app.selectAll()}>Select all<MenubarShortcut>Ctrl+A</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.range} onclick={() => app.clearSelection()}>Clear selection<MenubarShortcut>Esc</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.stats} onclick={() => void app.copyStats()}>Copy selection stats<MenubarShortcut>Ctrl+Shift+C</MenubarShortcut></MenubarItem>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>View</MenubarTrigger><MenubarContent>
    <MenubarItem disabled={!app.recording} onclick={() => app.zoomFit()}>Zoom to fit<MenubarShortcut>F</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.recording} onclick={() => app.zoom(.5)}>Zoom in<MenubarShortcut>+ / =</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.recording} onclick={() => app.zoom(2)}>Zoom out<MenubarShortcut>−</MenubarShortcut></MenubarItem>
    <MenubarCheckboxItem checked={app.follow} onclick={() => app.follow = !app.follow}>Follow</MenubarCheckboxItem>
    <MenubarItem onclick={() => app.drawerOpen = !app.drawerOpen}>Toggle bottom panel<MenubarShortcut>Ctrl+J</MenubarShortcut></MenubarItem>
    <MenubarSub><MenubarSubTrigger>Theme</MenubarSubTrigger><MenubarSubContent><MenubarRadioGroup value={app.theme}>{#each ['system','light','dark'] as theme}<MenubarRadioItem value={theme} onclick={() => app.setTheme(theme as 'system' | 'light' | 'dark')}>{theme[0].toUpperCase()+theme.slice(1)}</MenubarRadioItem>{/each}</MenubarRadioGroup></MenubarSubContent></MenubarSub>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>Device</MenubarTrigger><MenubarContent>
    <MenubarItem disabled={!app.connection.connected} onclick={() => void app.command('set_output',{on: !app.settings?.main})}>Output on/off<MenubarShortcut>Ctrl+E</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.connection.connected} onclick={() => void app.toggleRecording()}>{app.recording?.active ? 'Stop recording' : 'Start recording'}<MenubarShortcut>R</MenubarShortcut></MenubarItem>
    <MenubarItem disabled={!app.connection.connected} onclick={() => void app.toggleUart()}>UART {app.uartOpen ? 'close' : 'open'}</MenubarItem>
    <MenubarItem disabled={!app.connection.connected} onclick={() => app.dialog='device'}>Device info…</MenubarItem>
  </MenubarContent></MenubarMenu>
  <MenubarMenu><MenubarTrigger>Help</MenubarTrigger><MenubarContent>
    <MenubarItem onclick={() => app.showShortcuts()}>Keyboard shortcuts<MenubarShortcut>?</MenubarShortcut></MenubarItem>
    <MenubarItem onclick={() => app.dialog='about'}>About OpenOtii</MenubarItem>
  </MenubarContent></MenubarMenu>
</Menubar>
