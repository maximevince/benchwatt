<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { backend, type View, type UartChunk } from '$lib/backend';
  import { app } from '$lib/stores.svelte';
  import { formatReading } from '$lib/format';
  import { UART_LANE, formatValue, readingKind, type Lane } from '$lib/channels';
  import SelectionCard from './SelectionCard.svelte';
  import { niceTicks, zoomAt, panFrom, type Range } from '$lib/view-math';
  import { envelopeRuns, type EnvelopePoint } from '$lib/view';
  import { lineStartForChunk, linesFromChunks } from '$lib/uart';
  import { valuesAtTimeTsv } from '$lib/point-values';
  import ChannelName from './ChannelName.svelte';
  import { ContextMenu, ContextMenuTrigger, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuShortcut, ContextMenuSub, ContextMenuSubTrigger, ContextMenuSubContent, ContextMenuCheckboxItem } from '$lib/components/ui/context-menu';
  let host: HTMLDivElement, canvas: HTMLCanvasElement;
  let width = $state(700), availableHeight = $state(400), version = $state(0);
  let selecting = $state(false), cursor = $state<number | null>(null);
  let hoveredUart = $state<UartChunk | null>(null);
  let menuTime = $state(0), menuUart = $state<UartChunk | null>(null);
  let locked = $state<string[]>([]), logarithmic = $state<string[]>([]);
  let scales: Record<string, Range> = {}, visibleMeans: Record<string, number> = {}, sampleSpacing = Infinity, previousDuration = 0, wasActive = false, queued = false;
  const views = new Map<string, View>(), inflight = new Set<string>();
  let drag: { x: number; range: Range; mode: 'pan' | 'select' | 'start' | 'end'; anchor: number } | null = null;
  // Lanes come from the recording's own channel list, so an opened file plots without its device.
  const analog = $derived(app.lanes.filter(lane => lane.kind === 'analog').map(lane => lane.key));
  const lanes = $derived(app.lanes.filter(lane => !app.hiddenLanes.includes(lane.key)));
  const hasUart = $derived(lanes.some(lane => lane.kind === 'uart'));
  const analogHeight = $derived(Math.max(110, (availableHeight - 32 - lanes.filter(lane => lane.kind === 'digital').length * 24 - (hasUart ? 44 : 0)) / Math.max(analog.length, 1)));
  const laneHeight = (lane: Lane) => lane.kind === 'uart' ? 44 : lane.kind === 'digital' ? 24 : analogHeight;
  const height = $derived(32 + lanes.reduce((sum, lane) => sum + laneHeight(lane), 0));
  const topOf = (key: string) => 32 + lanes.slice(0, lanes.findIndex(lane => lane.key === key)).reduce((sum, lane) => sum + laneHeight(lane), 0);
  const descOf = (ch: string) => app.lanes.find(lane => lane.key === ch)?.channel;
  const labelOf = (key: string) => app.lanes.find(lane => lane.key === key)?.label ?? key;
  const x = (t: number) => (t - app.viewRange[0]) / (app.viewRange[1] - app.viewRange[0]) * width;
  const time = (px: number) => app.viewRange[0] + px / width * (app.viewRange[1] - app.viewRange[0]);
  function uartIndex(t: number) { let lo=0, hi=app.uart.length; while (lo<hi) { const mid=(lo+hi)>>1; if (app.uart[mid].t<t) lo=mid+1; else hi=mid; } return lo; }
  const minimumSpan = () => Math.max(1e-6, 20 * Math.min(sampleSpacing, (app.recording?.duration_s ?? 1) / Math.max(1, ...analog.map(ch => app.recording?.samples[ch] ?? 0))));
  const duration = () => Math.max(app.recording?.duration_s ?? 0, minimumSpan());
  function reading(value: number, ch: string) { const desc=descOf(ch); return desc ? formatValue(value, desc) : String(value); }
  function axisReading(value: number, step: number, ch: string) {
    const desc=descOf(ch), kind=desc && readingKind(desc.quantity);
    if (!kind) return (value.toFixed(Math.max(0, Math.ceil(-Math.log10(Math.abs(step) || 1))))+' '+(desc?.unit ?? '')).trim();
    const r = formatReading(value, kind);
    const factors: Record<string, number> = { A: 1, mA: 1e-3, µA: 1e-6, nA: 1e-9, W: 1, mW: 1e-3, µW: 1e-6, nW: 1e-9, V: 1, mV: 1e-3, µV: 1e-6 };
    const factor=factors[r.unit] ?? 1, scaledStep=Math.abs(step/factor);
    let decimals=0; while (decimals<12 && Math.abs(scaledStep*10**decimals-Math.round(scaledStep*10**decimals))>1e-8) decimals++;
    return (value/factor).toFixed(decimals)+' '+r.unit;
  }
  function currentValue(ch: string) {
    version; const view = views.get(ch);
    if (!view || cursor == null || !view.t.length) return '—';
    let i = view.t.findIndex(t => t >= cursor!); if (i < 0) i = view.t.length-1;
    return view.kind === 'raw' ? reading(view.v[i], ch) : reading(view.min[i], ch)+'…'+reading(view.max[i], ch);
  }
  function meanValue(ch: string) {
    version;
    return ch in visibleMeans ? reading(visibleMeans[ch],ch) : '—';
  }
  function formatTime(t: number, span: number) { return span < .01 ? (t*1e6).toFixed(0)+' µs' : span < 1 ? (t*1e3).toFixed(1)+' ms' : span < 120 ? t.toFixed(2)+' s' : (t/60).toFixed(2)+' min'; }
  function printable(bytes: number[]) { return bytes.map(byte => byte === 10 ? '↵' : byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : '·').join(''); }
  function hex(bytes: number[]) { return bytes.map(byte => byte.toString(16).padStart(2,'0').toUpperCase()).join(' '); }
  function schedule() { if (queued) return; queued = true; requestAnimationFrame(() => { queued = false; void fetchViews(); draw(); }); }
  async function fetchViews() {
    const [t0,t1] = app.viewRange;
    for (const ch of analog) {
      if (inflight.has(ch)) continue; inflight.add(ch);
      void backend.getView(ch,t0,t1,Math.max(1,Math.floor(width*devicePixelRatio/2))).then(view => {
        if (app.viewRange[0] === t0 && app.viewRange[1] === t1) {
          if (view.kind === 'raw') for (let i=1;i<view.t.length;i++) if (Number.isFinite(view.v[i]) && Number.isFinite(view.v[i-1]) && view.t[i]>view.t[i-1]) sampleSpacing=Math.min(sampleSpacing,view.t[i]-view.t[i-1]);
          views.set(ch,view); version++; draw();
        }
      }).catch(cause => app.report(cause)).finally(() => { inflight.delete(ch); if (app.viewRange[0] !== t0 || app.viewRange[1] !== t1) schedule(); });
    }
    void backend.getStats(t0,t1).then(stats => {
      if (app.viewRange[0] === t0 && app.viewRange[1] === t1) {
        visibleMeans=Object.fromEntries(Object.entries(stats.channels).map(([ch,stat]) => [ch,stat.mean]));
        version++;
      }
    }).catch(cause => app.report(cause));
    void app.refreshDetails();
  }
  function draw() {
    if (!canvas || width < 1 || height < 1) return;
    const ratio = devicePixelRatio || 1; canvas.width = Math.ceil(width*ratio); canvas.height = Math.ceil(height*ratio);
    const ctx = canvas.getContext('2d'); if (!ctx) return; ctx.scale(ratio,ratio);
    const css = getComputedStyle(canvas), color = (token: string) => css.getPropertyValue(token).trim();
    ctx.clearRect(0,0,width,height); ctx.font = '10px "Geist Mono"';
    const span = app.viewRange[1]-app.viewRange[0];
    for (let i=0;i<=5;i++) { const at=width*i/5; ctx.fillStyle=color('--muted-foreground'); ctx.fillText(formatTime(time(at),span),Math.min(at+4,width-70),16); ctx.strokeStyle=color('--border'); ctx.lineWidth=1; ctx.beginPath(); ctx.moveTo(at,24); ctx.lineTo(at,height); ctx.stroke(); }
    for (const lane of lanes) {
      const ch=lane.key, top=topOf(ch), bottom=top+laneHeight(lane);
      ctx.strokeStyle=color('--border'); ctx.lineWidth=1; ctx.beginPath(); ctx.moveTo(0,bottom-.5); ctx.lineTo(width,bottom-.5); ctx.stroke();
      if (lane.kind === 'digital') {
        const bit=lane.bit, low=bottom-5, high=top+4;
        let mask=app.edges[0]?.mask ?? 0, last=0;
        for (const edge of app.edges) {
          if (edge.t < app.viewRange[0]) { mask=edge.mask; continue; }
          if (edge.t > app.viewRange[1]) break;
          const xx=x(edge.t), oldY=mask & (1<<bit) ? high : low;
          ctx.strokeStyle=color('--digital'); ctx.beginPath(); ctx.moveTo(last,oldY); ctx.lineTo(xx,oldY); ctx.lineTo(xx,edge.mask & (1<<bit) ? high : low); ctx.stroke();
          last=xx; mask=edge.mask;
        }
        ctx.strokeStyle=color('--digital'); ctx.beginPath(); ctx.moveTo(last,mask & (1<<bit) ? high : low); ctx.lineTo(width,mask & (1<<bit) ? high : low); ctx.stroke(); continue;
      }
      if (lane.kind === 'uart') {
        // Two rows: rx on top, tx below. Each pill gets the room up to the next chunk in its own row.
        const rowTop = (dir: string | undefined) => top+(dir === 'tx' ? 23 : 4);
        const charWidth=ctx.measureText('M').width;
        const chunks: UartChunk[]=[]; for (let i=uartIndex(app.viewRange[0]); i<app.uart.length && app.uart[i].t<=app.viewRange[1]; i++) chunks.push(app.uart[i]);
        chunks.forEach((chunk, i) => {
          const tx=chunk.dir === 'tx', y=rowTop(chunk.dir), xx=x(chunk.t);
          const next=chunks.slice(i+1).find(item => (item.dir === 'tx') === tx);
          const room=(next ? x(next.t) : width)-xx-3;
          let label=printable(chunk.bytes);
          const fits=Math.floor((room-12)/charWidth);
          if (fits < label.length) label=fits >= 3 ? label.slice(0,fits-1)+'…' : '';
          ctx.fillStyle=color('--uart');
          if (label) {
            const w=ctx.measureText(label).width+12;
            ctx.globalAlpha=tx ? .1 : .18; ctx.fillRect(xx,y,w,16);
            ctx.globalAlpha=tx ? .6 : 1; ctx.fillText(label,xx+6,y+12);
          } else {
            ctx.strokeStyle=color('--uart'); ctx.globalAlpha=tx ? .5 : 1;
            ctx.beginPath(); ctx.moveTo(xx,y); ctx.lineTo(xx,y+16); ctx.stroke();
          }
          ctx.globalAlpha=1;
        });
        continue;
      }
      const view=views.get(ch); if (!view || !view.t.length) continue;
      const values=(view.kind === 'raw' ? view.v : [...view.min,...view.max]).filter(Number.isFinite); if (!values.length) continue;
      const log=logarithmic.includes(ch), transform=(v: number) => log ? Math.log10(Math.max(1e-9,v)) : v;
      const transformed=values.map(transform); let min=Math.min(...transformed), max=Math.max(...transformed);
      if (locked.includes(ch) && scales[ch]) [min,max]=scales[ch];
      else {
        const dataMin=min, dataMax=max;
        const pad=Math.max((max-min)*.1,log ? .05 : Math.max(Math.abs(min),Math.abs(max))*.01,1e-9);
        min-=pad; max+=pad;
        if (!log && dataMin >= 0) min=Math.max(0,min);
        if (!log && dataMax <= 0) max=Math.min(0,max);
        const ticks=niceTicks(min,max,5);
        if (ticks.length > 1) [min,max]=[ticks[0],ticks.at(-1)!];
        scales[ch]=[min,max];
      }
      const y=(v: number) => bottom-8-(transform(v)-min)/(max-min)*(laneHeight(lane)-16);
      const ticks=niceTicks(min,max,5);
      const tickStep=ticks.length > 1 ? ticks[1]-ticks[0] : max-min;
      const eps=(max-min)*1e-6;
      for (const tick of ticks.filter(t => t >= min-eps && t <= max+eps)) { const yy=bottom-8-(tick-min)/(max-min)*(laneHeight(lane)-16); ctx.strokeStyle=color('--border'); ctx.beginPath(); ctx.moveTo(0,yy); ctx.lineTo(width,yy); ctx.stroke(); ctx.fillStyle=color('--muted-foreground'); ctx.fillText(log ? reading(10**tick,ch) : axisReading(tick,tickStep,ch),4,yy-3); }
      const trace=color('--'+lane.color); ctx.strokeStyle=trace; ctx.fillStyle=trace;
      if (view.kind === 'raw') {
        ctx.lineWidth=1.25; ctx.beginPath(); let lastValid=false;
        view.t.forEach((t,i) => { if (!Number.isFinite(view.v[i])) { lastValid=false; return; } if (lastValid) ctx.lineTo(x(t),y(view.v[i])); else ctx.moveTo(x(t),y(view.v[i])); lastValid=true; }); ctx.stroke();
      } else {
        const traceBoundary=(points: EnvelopePoint[]) => {
          points.forEach((point,i) => i ? ctx.lineTo(point.x,y(point.value)) : ctx.moveTo(point.x,y(point.value)));
        };
        for (const run of envelopeRuns(view,app.viewRange,width)) {
          ctx.beginPath(); traceBoundary(run.upper);
          ctx.lineTo(run.upper.at(-1)!.x,y(run.baseline));
          ctx.lineTo(run.upper[0].x,y(run.baseline));
          ctx.closePath(); ctx.globalAlpha=.45; ctx.fill(); ctx.globalAlpha=1;
          ctx.beginPath(); traceBoundary(run.upper); traceBoundary(run.lower); ctx.lineWidth=1; ctx.stroke();
        }
      }
    }
    if (app.range) {
      const a=x(app.range[0]),b=x(app.range[1]); ctx.fillStyle='rgba(56,189,248,.16)'; ctx.fillRect(a,24,b-a,height-24);
      ctx.strokeStyle=color('--voltage'); ctx.lineWidth=1; for (const edge of [a,b]) { ctx.beginPath(); ctx.moveTo(edge,24); ctx.lineTo(edge,height); ctx.stroke(); }
      ctx.fillStyle=color('--foreground'); ctx.fillText(formatTime(app.range[1]-app.range[0],app.range[1]-app.range[0]),Math.max(4,Math.min(a+5,width-80)),30);
    }
    const marker=app.hoverMarker ?? app.marker; if (marker != null) { const xx=x(marker); ctx.strokeStyle=color('--uart'); ctx.lineWidth=1; ctx.beginPath(); ctx.moveTo(xx+.5,24); ctx.lineTo(xx+.5,height); ctx.stroke(); }
    if (cursor != null) { const xx=x(cursor); ctx.strokeStyle=color('--foreground'); ctx.lineWidth=1; ctx.beginPath(); ctx.moveTo(xx+.5,24); ctx.lineTo(xx+.5,height); ctx.stroke(); const left=Math.max(0,Math.min(xx+4,width-82)); ctx.fillStyle=color('--background'); ctx.fillRect(left,1,80,19); ctx.fillStyle=color('--foreground'); ctx.fillText(formatTime(cursor,span),left+2,15); }
  }
  function wheel(event: WheelEvent) { event.preventDefault(); app.follow=false; app.viewRange=zoomAt(app.viewRange,time(event.offsetX),Math.exp(event.deltaY*.001),duration(),minimumSpan()); schedule(); }
  function toggleLock(ch: string) { locked=locked.includes(ch) ? locked.filter(x => x !== ch) : [...locked,ch]; draw(); }
  function toggleLog(ch: string) { logarithmic=logarithmic.includes(ch) ? logarithmic.filter(x => x !== ch) : [...logarithmic,ch]; delete scales[ch]; draw(); }
  function autoscale(ch: string) { locked=locked.filter(x => x !== ch); delete scales[ch]; draw(); }
  function zoomHere(factor: number) { app.follow=false; app.viewRange=zoomAt(app.viewRange,menuTime,factor,duration(),minimumSpan()); schedule(); }
  function centerHere() { const span=app.viewRange[1]-app.viewRange[0]; app.follow=false; app.viewRange=panFrom(app.viewRange,(app.viewRange[0]+span/2-menuTime)/span*width,width,duration(),minimumSpan()); schedule(); }
  async function copyPoint() { try { await navigator.clipboard.writeText(valuesAtTimeTsv(menuTime,analog,views)); } catch (cause) { app.report(cause); } }
  function uartAt(px: number, py: number) {
    if (!hasUart || py < topOf(UART_LANE) || py > topOf(UART_LANE)+44) return null;
    const tx=py > topOf(UART_LANE)+21;
    return app.uart.slice(Math.max(0,uartIndex(time(px))-8),uartIndex(time(px))+2).filter(chunk => (chunk.dir === 'tx') === tx && px >= x(chunk.t)-6 && px <= x(chunk.t)+Math.max(8,printable(chunk.bytes).length*6+12)).at(-1) ?? null;
  }
  function plotContext(event: MouseEvent) { menuTime=Math.max(0,Math.min(duration(),time(event.offsetX))); menuUart=uartAt(event.offsetX,event.offsetY); }
  function showUartInConsole() { if (!menuUart) return; const start=lineStartForChunk(app.uart,app.uart.indexOf(menuUart)); if (start === null) return; app.focusedUartTime=start; app.drawerTab='uart'; app.drawerOpen=true; }
  function contextUartLine() { if (!menuUart) return null; const start=lineStartForChunk(app.uart,app.uart.indexOf(menuUart)); return linesFromChunks(app.uart).find(line => line.t === start && line.dir === menuUart?.dir) ?? null; }
  function copyUartLine(asHex = false) { const line=contextUartLine(); if (!line) return; const value=asHex ? line.bytes.map(byte => byte.toString(16).padStart(2,'0').toUpperCase()).join(' ') : line.text; void navigator.clipboard.writeText(value); }
  function down(event: PointerEvent) {
    if (event.button !== 0) return;
    try { canvas.setPointerCapture(event.pointerId); } catch { /* Synthetic pointer events have no active pointer. */ }
    const at=time(event.offsetX);
    const start=app.range && Math.abs(x(app.range[0])-event.offsetX)<7, end=app.range && Math.abs(x(app.range[1])-event.offsetX)<7;
    const mode=start ? 'start' : end ? 'end' : event.shiftKey || selecting ? 'select' : 'pan';
    drag={x:event.offsetX,range:[...app.viewRange],mode,anchor:at}; if (mode === 'select') { app.follow=false; app.range=[at,at]; }
  }
  function move(event: PointerEvent) {
    cursor=Math.max(0,Math.min(duration(),time(event.offsetX))); hoveredUart=null;
    if (drag) {
      if (drag.mode === 'select') app.range=[Math.min(drag.anchor,cursor),Math.max(drag.anchor,cursor)];
      else if (drag.mode === 'start' && app.range) app.range=[Math.min(cursor,app.range[1]),app.range[1]];
      else if (drag.mode === 'end' && app.range) app.range=[app.range[0],Math.max(cursor,app.range[0])];
      else { app.follow=false; app.viewRange=panFrom(drag.range,event.offsetX-drag.x,width,duration(),minimumSpan()); }
      schedule();
    } else { hoveredUart=uartAt(event.offsetX,event.offsetY); app.focusedUartTime=hoveredUart ? lineStartForChunk(app.uart,app.uart.indexOf(hoveredUart)) : null; draw(); }
  }
  function up(event: PointerEvent) { if (event.type === 'pointerup' && event.button !== 0) return; drag=null; if (app.range && app.range[1]-app.range[0]<1e-6) app.range=null; void app.refreshDetails(); }
  function zoomToSelection() {
    if (!app.range) return;
    const pad=(app.range[1]-app.range[0])*.05;
    app.follow=false; app.viewRange=[Math.max(0,app.range[0]-pad),Math.min(duration(),app.range[1]+pad)];
  }
  function fit() { app.follow=false; app.viewRange=[0,duration()]; schedule(); }
  function keydown(event: KeyboardEvent) { if (event.key === 'Escape') { app.range=null; void app.refreshDetails(); draw(); } }
  onMount(() => { const observer=new ResizeObserver(() => { width=Math.max(1,host.clientWidth-150); availableHeight=Math.max(1,host.clientHeight); schedule(); }); observer.observe(host); window.addEventListener('keydown',keydown); return () => { observer.disconnect(); window.removeEventListener('keydown',keydown); }; });
  // Recording progress may move the view (follow/fit); untracked so writing viewRange can't re-trigger this effect.
  $effect(() => {
    const recording=app.recording;
    untrack(() => {
      const elapsed=recording?.duration_s ?? 0;
      if (elapsed<previousDuration) { views.clear(); scales={}; visibleMeans={}; sampleSpacing=Infinity; app.follow=true; }
      if (wasActive && recording && !recording.active) fit();
      wasActive=recording?.active ?? false; previousDuration=elapsed;
      if (app.follow && recording?.active) { const span=app.viewRange[1]-app.viewRange[0]; if (elapsed>span) app.viewRange=[elapsed-span,elapsed]; }
      schedule();
    });
  });
  $effect(() => { app.viewRange; app.theme; schedule(); });
  $effect(() => { app.edges; app.marker; app.hoverMarker; height; if (canvas) draw(); });
  $effect(() => { const range=app.range; if (!range || range[0] === range[1]) return; const frame=requestAnimationFrame(() => void app.refreshStats()); return () => cancelAnimationFrame(frame); });
</script>
<section class="recording-panel">
  <div class="recording-toolbar"><strong>Recording</strong><span>{app.recording?.duration_s.toFixed(2) ?? '0.00'} s · {app.recording?.gaps ?? 0} gaps</span><button onclick={() => { app.follow=!app.follow; schedule(); }} class:active={app.follow}>Follow</button><button onclick={() => selecting=!selecting} class:active={selecting}>Select range</button><button onclick={fit}>Fit</button></div>
  <div class="recording-stage" bind:this={host}>
    <div class="lane-headers"><div class="axis-spacer"></div>{#each lanes as lane (lane.key)}{@const ch=lane.key}{@const isAnalog=lane.kind === 'analog'}{@const isCurrent=isAnalog && lane.channel.quantity === 'Current'}
      <ContextMenu><ContextMenuTrigger class="lane-label" style={'height:'+laneHeight(lane)+'px'}><div class="lane-name"><strong title={lane.channel.name}><ChannelName label={lane.label} color={lane.color} output={lane.output}/></strong><small>{isAnalog ? lane.unit : ''}</small></div>
        {#if isAnalog}<div class="lane-value">{currentValue(ch)}</div><div class="lane-mean">mean {meanValue(ch)}</div><div class="lane-actions"><button onclick={() => toggleLock(ch)} title="Lock y scale" aria-label={'Lock y scale for '+lane.label}>{locked.includes(ch) ? '🔒' : '⌁'}</button>{#if isCurrent}<button class:active={logarithmic.includes(ch)} onclick={() => toggleLog(ch)} title="Log scale" aria-label={'Log scale for '+lane.label}>log</button>{/if}</div>{/if}</ContextMenuTrigger>
        <ContextMenuContent class="plot-context-menu"><ContextMenuCheckboxItem disabled={!isAnalog} checked={locked.includes(ch)} onclick={() => toggleLock(ch)}>Lock y scale</ContextMenuCheckboxItem><ContextMenuCheckboxItem disabled={!isCurrent} checked={logarithmic.includes(ch)} onclick={() => toggleLog(ch)}>Log scale</ContextMenuCheckboxItem><ContextMenuItem disabled={!isAnalog} onclick={() => autoscale(ch)}>Autoscale now</ContextMenuItem><ContextMenuSeparator/><ContextMenuItem onclick={() => app.hiddenLanes=[...app.hiddenLanes,ch]}>Hide lane</ContextMenuItem><ContextMenuSub><ContextMenuSubTrigger disabled={!app.hiddenLanes.length}>Hidden lanes</ContextMenuSubTrigger><ContextMenuSubContent class="plot-context-menu">{#each app.hiddenLanes as hidden}<ContextMenuItem onclick={() => app.hiddenLanes=app.hiddenLanes.filter(key => key !== hidden)}>{labelOf(hidden)}</ContextMenuItem>{/each}</ContextMenuSubContent></ContextMenuSub></ContextMenuContent></ContextMenu>
    {/each}</div>
    <div class="plot-surface"><ContextMenu><ContextMenuTrigger><canvas bind:this={canvas} style:width={width+'px'} style:height={height+'px'} onwheel={wheel} oncontextmenu={plotContext} onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up} ondblclick={fit} onpointerleave={() => { cursor=null; hoveredUart=null; draw(); }}></canvas></ContextMenuTrigger>
      <ContextMenuContent class="plot-context-menu">{#if menuUart}<ContextMenuItem onclick={() => copyUartLine()}>Copy line</ContextMenuItem><ContextMenuItem onclick={() => copyUartLine(true)}>Copy as hex</ContextMenuItem><ContextMenuItem onclick={showUartInConsole}>Show in console</ContextMenuItem><ContextMenuSeparator/>{/if}<ContextMenuItem disabled={!app.range} onclick={zoomToSelection}>Zoom to selection</ContextMenuItem><ContextMenuItem disabled={!app.range || !app.stats} onclick={() => void app.copyStats()}>Copy selection stats<ContextMenuShortcut>Ctrl+Shift+C</ContextMenuShortcut></ContextMenuItem><ContextMenuSub><ContextMenuSubTrigger disabled={!app.range}>Export selection</ContextMenuSubTrigger><ContextMenuSubContent class="plot-context-menu"><ContextMenuItem onclick={() => void app.exportRecording('csv',true)}>CSV…</ContextMenuItem><ContextMenuItem onclick={() => void app.exportRecording('sr',true)}>sigrok session (.sr)…</ContextMenuItem></ContextMenuSubContent></ContextMenuSub><ContextMenuItem disabled={!app.range} onclick={() => app.clearSelection()}>Clear selection<ContextMenuShortcut>Esc</ContextMenuShortcut></ContextMenuItem><ContextMenuSeparator/><ContextMenuItem onclick={() => app.zoomFit()}>Zoom to fit<ContextMenuShortcut>F</ContextMenuShortcut></ContextMenuItem><ContextMenuItem onclick={() => zoomHere(.5)}>Zoom in here</ContextMenuItem><ContextMenuItem onclick={() => zoomHere(2)}>Zoom out here</ContextMenuItem><ContextMenuItem onclick={centerHere}>Center view here</ContextMenuItem><ContextMenuItem onclick={() => void copyPoint()}>Copy time and values at this point</ContextMenuItem></ContextMenuContent></ContextMenu>
      {#if app.range}<SelectionCard range={app.range} viewRange={app.viewRange} {width} stats={app.stats}/>{/if}
      {#if hoveredUart}<div class="uart-tooltip" style:left={Math.min(width-180,Math.max(4,x(hoveredUart.t)+8))+'px'} style:top={topOf(UART_LANE)-4+'px'} style:transform="translateY(-100%)"><time>{hoveredUart.t.toFixed(6)} s</time><span>{printable(hoveredUart.bytes)}</span><code>{hex(hoveredUart.bytes)}</code></div>{/if}
    </div>
  </div>
</section>
