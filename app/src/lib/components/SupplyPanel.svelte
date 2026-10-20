<script lang="ts">
  import { app } from '$lib/stores.svelte';
  import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '$lib/components/ui/select';
  import { Separator } from '$lib/components/ui/separator';
  import { Switch } from '$lib/components/ui/switch';
  import Readout from './Readout.svelte';
  import SetpointInput from './SetpointInput.svelte';
  let max = $derived(app.settings?.mainvolt_max ?? 3.76);
</script>
<section class="sidebar-section supply" class:output-on={app.settings?.main} aria-label="Supply">
  <div class="section-heading"><h2>Supply</h2><span class="section-meta">MAIN OUTPUT</span></div>
  <div class="hero-readouts" class:dimmed={!app.connection.connected}>
    <Readout label="VOLTAGE" value={app.monitor.mv} kind="voltage" />
    <Readout label="CURRENT" value={app.monitor.mc} kind="current" />
    <Readout label="POWER" value={app.monitor.mp} kind="power" />
  </div>
  <Separator class="supply-separator" />
  <div class="output-row">
    <div><strong>Output</strong><span>{app.settings?.main ? 'Enabled' : 'Disabled'}</span></div>
    <Switch checked={app.settings?.main ?? false} disabled={app.disabled} onCheckedChange={on => void app.command('set_output', { on })} aria-label="Toggle output" />
  </div>
  <div class="setpoint-list">
    <SetpointInput label="Voltage" value={app.settings?.mainvolt ?? null} suffix="V" hint={`max ${max.toFixed(2)} V`} min={0} {max} step={0.001} precision={3} disabled={app.disabled} apply={v => app.command('set_voltage', { volts: v })} />
    <SetpointInput label="Current limit" value={app.settings?.maxcur == null ? null : app.settings.maxcur * 1000} suffix="mA" hint="1–3000 mA" min={1} max={3000} step={1} precision={0} disabled={app.disabled} apply={v => app.command('set_current_limit', { amps: v / 1000 })} />
  </div>
  <div class="mode-row"><span>Current mode</span><Select type="single" value={app.settings?.mainsrccurrmode === 1 ? 'limit' : 'cutoff'} onValueChange={mode => { if (mode) void app.command('set_current_mode', { mode }); }} disabled={app.disabled}><SelectTrigger size="sm" aria-label="Current mode"><SelectValue placeholder="Cutoff" /></SelectTrigger><SelectContent><SelectItem value="cutoff">Cutoff</SelectItem><SelectItem value="limit">Limit</SelectItem></SelectContent></Select></div>
</section>
