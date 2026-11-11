<script lang="ts">
  import type { OutputTag } from '$lib/channels';
  // A channel's colour swatch and label. A channel of a coloured output shows the output as a chip in that colour instead of the swatch.
  let { label, color, output }: { label: string; color: string; output: OutputTag | null } = $props();
</script>
{#if output?.color}<span class="output-chip marked" style:--output={'var(--' + output.color + ')'}>{output.label}</span>{:else}<i class="swatch" style:--channel-color={'var(--' + color + ')'}></i>{#if output}<span class="output-chip">{output.label}</span>{/if}{/if}<span class="channel-name">{output ? output.rest : label}</span>
