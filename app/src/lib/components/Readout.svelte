<script lang="ts">
  import { formatReading } from '$lib/format';
  let { label, value, kind, set = false }: { label: string; value: number | null; kind: 'current' | 'voltage' | 'power'; set?: boolean } = $props();
  let reading = $derived(formatReading(value, kind));
</script>
<div class="readout" data-channel={kind}>
  <div class="readout-label">{label}{#if set}<span class="readout-set" title="Setpoint, not measured">SET</span>{/if}</div>
  <div class="readout-value"><span>{reading.number}</span><small>{reading.unit}</small></div>
</div>
