<script lang="ts">
  import { formatReading } from '$lib/format';
  let { label, value, kind }: { label: string; value: number | null; kind: 'current' | 'voltage' | 'power' } = $props();
  let reading = $derived(formatReading(value, kind));
</script>
<div class="readout" data-channel={kind}>
  <div class="readout-label">{label}</div>
  <div class="readout-value"><span>{reading.number}</span><small>{reading.unit}</small></div>
</div>
