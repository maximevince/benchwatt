<script lang="ts">
  import { app, type Tab } from '$lib/stores.svelte';
  import { Tooltip, TooltipContent, TooltipTrigger } from '$lib/components/ui/tooltip';
  import { FolderOpen, Plus, X } from 'lucide-svelte';
  // One tab per open recording: the live one, earlier ones and opened files.
  const clock = (s: number) => `${Math.floor(s / 60).toString().padStart(2, '0')}:${Math.floor(s % 60).toString().padStart(2, '0')}`;
  function auxClose(event: MouseEvent, tab: Tab) { if (event.button === 1) { event.preventDefault(); void app.closeTab(tab); } }
  function about(tab: Tab) {
    const { model, serial } = tab.info.source;
    return [tab.path || 'Not saved yet', [model, serial].filter(Boolean).join(' · ')].filter(Boolean).join('\n');
  }
</script>
<div class="tab-strip" role="tablist" aria-label="Recordings">
  {#each app.tabs as tab (tab.id)}
    <div class="rec-tab" class:current={tab === app.tab} title={about(tab)} onauxclick={event => auxClose(event, tab)} role="presentation">
      <button class="rec-tab-main" role="tab" aria-selected={tab === app.tab} onclick={() => app.showTab(tab)}>
        {#if tab.info.active}<span class="record-dot record-pulse"></span>{/if}
        <span class="rec-tab-title">{tab.title}</span>
        {#if tab.fileName && tab.source}<span class="rec-tab-source">{tab.source}</span>{/if}
        {#if tab.info.active}<span class="rec-tab-source">{clock(tab.info.duration_s)}</span>{:else if tab.unsaved}<i class="unsaved-dot" title="Not saved"></i>{/if}
      </button>
      <button class="rec-tab-close" aria-label={`Close ${tab.title}`} onclick={() => void app.closeTab(tab)}><X size={13}/></button>
    </div>
  {/each}
  <Tooltip><TooltipTrigger><button class="tab-action" disabled={!app.selected || !!app.live} onclick={() => void app.toggleRecording()} aria-label="New recording"><Plus size={15}/></button></TooltipTrigger><TooltipContent>New recording (R)</TooltipContent></Tooltip>
  <Tooltip><TooltipTrigger><button class="tab-action" onclick={() => void app.openRecording()} aria-label="Open recording"><FolderOpen size={14}/></button></TooltipTrigger><TooltipContent>Open recording… (Ctrl+O)</TooltipContent></Tooltip>
</div>
