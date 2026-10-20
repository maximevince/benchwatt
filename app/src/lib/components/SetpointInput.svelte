<script lang="ts">
  import { Input } from '$lib/components/ui/input';
  import { app } from '$lib/stores.svelte';
  let { label, value, suffix, hint, min, max, step, disabled, apply }: { label: string; value: number | null; suffix: string; hint: string; min: number; max: number; step: number; disabled: boolean; apply: (value: number) => Promise<boolean> } = $props();
  let draft = $state('');
  let confirmed = $state(false);
  let editing = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => { if (!editing) draft = value == null ? '' : String(value); });
  async function commit() {
    editing = false;
    if (disabled || value == null || draft.trim() === String(value)) return;
    const parsed = Number(draft);
    if (!draft.trim() || !Number.isFinite(parsed) || parsed < min || parsed > max) { app.report(`${label} must be between ${min} and ${max} ${suffix}.`); draft = String(value); return; }
    if (await apply(parsed)) { confirmed = true; clearTimeout(timer); timer = setTimeout(() => confirmed = false, 750); }
    else draft = String(value);
  }
</script>
<label class="setpoint">
  <span class="field-title">{label}<span class="field-hint">{hint}</span></span>
  <span class="input-wrap" class:confirmed>
    <Input type="number" {min} {max} {step} {disabled} bind:value={draft} onfocus={() => editing = true} onblur={() => void commit()} onkeydown={e => { if (e.key === 'Enter') e.currentTarget.blur(); }} />
    <span class="suffix">{suffix}</span>
  </span>
</label>
