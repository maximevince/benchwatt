<script lang="ts">
  import type { SettingDesc } from '$lib/backend';
  import { app } from '$lib/stores.svelte';
  import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '$lib/components/ui/select';
  import { Switch } from '$lib/components/ui/switch';
  import SetpointInput from './SetpointInput.svelte';
  import { numberField, optionLabel, settingError } from '$lib/channels';
  // One device setting, edited by its type; readonly ones are shown as text.
  let { setting, value }: { setting: SettingDesc; value: boolean | string | number | null } = $props();
  const field = $derived(numberField(setting));
  async function apply(next: boolean | string | number) {
    const error = settingError(setting, next);
    if (error) { app.report(error); return false; }
    return app.command('set_setting', { key: setting.key, value: next });
  }
  const text = (v: typeof value) => v == null ? '—' : typeof v === 'boolean' ? (v ? 'On' : 'Off') : typeof v === 'number' ? v.toFixed(field.precision) + (field.unit ? ' ' + field.unit : '') : optionLabel(v);
</script>
{#if setting.readonly}<div class="mode-row"><span>{setting.label}</span><code class="setting-value">{text(value)}</code></div>
{:else if setting.type === 'bool'}<div class="mode-row"><span>{setting.label}</span><Switch checked={value === true} disabled={app.disabled || value == null} onCheckedChange={on => void apply(on)} aria-label={setting.label} /></div>
{:else if setting.type === 'choice'}<div class="mode-row"><span>{setting.label}</span><Select type="single" value={typeof value === 'string' ? value : undefined} onValueChange={v => { if (v && v !== value) void apply(v); }} disabled={app.disabled || value == null}><SelectTrigger size="sm" aria-label={setting.label}><SelectValue placeholder="—" /></SelectTrigger><SelectContent>{#each setting.options ?? [] as option}<SelectItem value={option}>{optionLabel(option)}</SelectItem>{/each}</SelectContent></Select></div>
{:else}<SetpointInput label={setting.label} value={typeof value === 'number' ? value : null} suffix={field.unit} hint={field.hint} min={field.min} max={field.max} step={field.step} precision={field.precision} disabled={app.disabled} apply={apply} />{/if}
