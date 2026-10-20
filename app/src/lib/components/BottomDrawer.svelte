<script lang="ts">
  import { app } from '$lib/stores.svelte';
  let { onToggle }: { onToggle: () => void } = $props();
  import { Tabs, TabsList, TabsTrigger, TabsContent } from '$lib/components/ui/tabs';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { ScrollArea } from '$lib/components/ui/scroll-area';
  import { ChevronDown, ChevronUp } from 'lucide-svelte';
  import AlertsLog from './AlertsLog.svelte';
</script>
<section class="drawer" class:closed={!app.drawerOpen} aria-label="Bottom drawer"><Tabs value="alerts" class="drawer-tabs"><div class="drawer-header"><TabsList><TabsTrigger value="alerts">Alerts <Badge variant="secondary">{app.alerts.length}</Badge></TabsTrigger><TabsTrigger value="uart">UART</TabsTrigger><TabsTrigger value="stats">Selection stats</TabsTrigger></TabsList><Button variant="ghost" size="icon" onclick={onToggle} aria-label={app.drawerOpen ? 'Collapse drawer' : 'Expand drawer'}>{#if app.drawerOpen}<ChevronDown size={16}/>{:else}<ChevronUp size={16}/>{/if}</Button></div>{#if app.drawerOpen}<div class="drawer-content"><TabsContent value="alerts"><ScrollArea class="drawer-scroll"><AlertsLog /></ScrollArea></TabsContent><TabsContent value="uart"><p class="empty-state">UART RX will appear when recording is available.</p></TabsContent><TabsContent value="stats"><p class="empty-state">Select a range in a recording to view statistics.</p></TabsContent></div>{/if}</Tabs></section>
