<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { formatStatus, statusChannels } from '$lib/channels';
  import SettingField from './SettingField.svelte';
  // Device-wide settings, then live readings that belong to no output.
  const desc = $derived(app.selected?.desc);
  const settings = $derived(desc?.settings.filter(s => s.output === null) ?? []);
  const readings = $derived(app.monitor?.readings ?? {});
  const status = $derived(desc ? statusChannels(desc, readings) : []);
  const value = (key: string) => app.selected?.state?.settings[key] ?? null;
</script>
{#if settings.length || status.length}<section class="sidebar-section status" aria-label="Status"><div class="section-heading"><h2>Status</h2></div>
  {#if settings.some(s => !s.readonly)}<div class="setpoint-list status-settings">{#each settings.filter(s => !s.readonly) as setting (setting.key)}<SettingField {setting} value={value(setting.key)} />{/each}</div>{/if}
  <dl>{#each settings.filter(s => s.readonly) as setting (setting.key)}{@const v = value(setting.key)}<div><dt>{setting.label}</dt><dd>{v == null ? '—' : typeof v === 'boolean' ? (v ? 'On' : 'Off') : typeof v === 'number' ? v.toFixed(2) + (setting.unit ? ' ' + setting.unit : '') : v}</dd></div>{/each}{#each status as ch (ch.name)}<div><dt>{ch.label}</dt><dd>{formatStatus(readings[ch.name], ch)}</dd></div>{/each}</dl>
</section>{/if}
