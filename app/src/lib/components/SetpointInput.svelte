<script lang="ts">
  import { Input } from '$lib/components/ui/input';
  import { app } from '$lib/stores.svelte';
  let { label, value, suffix, hint, min, max, step, precision, disabled, apply }: { label: string; value: number | null; suffix: string; hint: string; min: number; max: number; step: number; precision: number; disabled: boolean; apply: (value: number) => Promise<boolean> } = $props();
  let draft = $state<string | number>('');
  let confirmed = $state(false);
  let editing = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deviceValue = () => value == null ? '' : value.toFixed(precision);
  $effect(() => { if (!editing) draft = deviceValue(); });
  async function commit() {
    editing = false;
    if (disabled || value == null) return;
    const text = String(draft).trim();
    const parsed = Number(text);
    if (!text || !Number.isFinite(parsed) || parsed < min || parsed > max) { app.report(`${label} must be between ${min} and ${max} ${suffix}.`); draft = deviceValue(); return; }
    const rounded = Number(parsed.toFixed(precision));
    if (rounded === Number(value.toFixed(precision))) { draft = deviceValue(); return; }
    if (await apply(rounded)) { confirmed = true; clearTimeout(timer); timer = setTimeout(() => confirmed = false, 750); }
    else draft = deviceValue();
  }
</script>
<label class="setpoint">
  <span class="field-title">{label}<span class="field-hint">{hint}</span></span>
  <span class="input-wrap" class:confirmed>
    <Input type="number" {min} {max} {step} {disabled} bind:value={draft} onfocus={() => editing = true} onblur={() => void commit()} onkeydown={e => { if (e.key === 'Enter') e.currentTarget.blur(); }} />
    <span class="suffix">{suffix}</span>
  </span>
</label>
