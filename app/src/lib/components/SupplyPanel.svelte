<script lang="ts">
  import type { Device } from '$lib/backend';
  import { app } from '$lib/stores.svelte';
  import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '$lib/components/ui/select';
  import { Separator } from '$lib/components/ui/separator';
  import { Switch } from '$lib/components/ui/switch';
  import Readout from './Readout.svelte';
  import SetpointInput from './SetpointInput.svelte';
  import SettingField from './SettingField.svelte';
  import { currentField, outputReadouts, readingKind, voltageField } from '$lib/channels';
  // One output of a device: its readouts, switch, setpoints and the settings that belong to it.
  let { device, index }: { device: Device; index: number } = $props();
  const output = $derived(device.desc.outputs[index]);
  const state = $derived(device.state?.outputs[index] ?? null);
  const readings = $derived(app.monitors[device.desc.id]?.readings ?? {});
  const volts = $derived(voltageField(output.voltage));
  const limit = $derived(output.current_limit && currentField(output.current_limit));
  const steps = $derived(output.voltage_steps.map(String));
</script>
<section class="sidebar-section supply" class:output-on={state?.enabled} aria-label={'Supply ' + output.label}>
  <div class="section-heading"><h2>Supply</h2><span class="section-meta">{output.label.toUpperCase()}</span></div>
  <div class="hero-readouts" class:dimmed={!device.state}>
    {#each outputReadouts(device.desc, output) as ch (ch.name)}<Readout label={ch.quantity.toUpperCase()} value={readings[ch.name] ?? null} kind={readingKind(ch.quantity) ?? 'voltage'} set={ch.set} />{/each}
  </div>
  <Separator class="supply-separator" />
  <div class="output-row">
    <div><strong>Output</strong><span>{state?.enabled ? 'Enabled' : 'Disabled'}</span></div>
    <Switch checked={state?.enabled ?? false} disabled={app.disabled} onCheckedChange={on => void app.command('set_output', { out: output.n, on })} aria-label={device.desc.outputs.length > 1 ? 'Toggle ' + output.label : 'Toggle output'} />
  </div>
  <div class="setpoint-list">
    {#if steps.length}<div class="mode-row"><span>Voltage</span><Select type="single" value={state?.voltage != null ? String(state.voltage) : undefined} onValueChange={v => { if (v) void app.command('set_voltage', { out: output.n, volts: Number(v) }); }} disabled={app.disabled}><SelectTrigger size="sm" aria-label="Voltage"><SelectValue placeholder="—" /></SelectTrigger><SelectContent>{#each steps as step}<SelectItem value={step}>{step} V</SelectItem>{/each}</SelectContent></Select></div>
    {:else}<SetpointInput label="Voltage" value={state?.voltage ?? null} suffix="V" hint={volts.hint} min={output.voltage.min} max={output.voltage.max} step={volts.step} precision={volts.precision} disabled={app.disabled} apply={v => app.command('set_voltage', { out: output.n, volts: v })} />{/if}
    {#if limit}<SetpointInput label="Current limit" value={state?.current_limit == null ? null : state.current_limit * limit.scale} suffix={limit.unit} hint={limit.hint} min={limit.min} max={limit.max} step={limit.step} precision={limit.precision} disabled={app.disabled} apply={v => app.command('set_current_limit', { out: output.n, amps: v / limit.scale })} />{/if}
  </div>
  {#each device.desc.settings.filter(s => s.output === output.n) as setting (setting.key)}{#if setting.type === 'number' && !setting.readonly}<div class="setpoint-list supply-setting"><SettingField {setting} value={device.state?.settings[setting.key] ?? null} /></div>{:else}<SettingField {setting} value={device.state?.settings[setting.key] ?? null} />{/if}{/each}
</section>
