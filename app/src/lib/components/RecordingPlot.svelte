<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { backend, type View, type UartChunk } from '$lib/backend';
  import { app } from '$lib/stores.svelte';
  import { formatReading } from '$lib/format';
  import { activeGpi, niceTicks, zoomAt, panFrom, type Range } from '$lib/view-math';
  import { envelopeRuns, type EnvelopePoint } from '$lib/view';
  import { lineStartForChunk } from '$lib/uart';
  let host: HTMLDivElement, canvas: HTMLCanvasElement;
  let width = $state(700), availableHeight = $state(400), version = $state(0);
  let selecting = $state(false), cursor = $state<number | null>(null);
  let expandedIdle = $state(false), hoveredUart = $state<UartChunk | null>(null);
  let locked = $state<string[]>([]), logarithmic = $state<string[]>([]);
  let scales: Record<string, Range> = {}, visibleMeans: Record<string, number> = {}, sampleSpacing = Infinity, previousDuration = 0, wasActive = false, queued = false;
  const views = new Map<string, View>(), inflight = new Set<string>();
  let drag: { x: number; range: Range; mode: 'pan' | 'select' | 'start' | 'end'; anchor: number } | null = null;
  const colors: Record<string, string> = { mc: 'var(--current)', mv: 'var(--voltage)', mp: 'var(--power)', ac: 'var(--teal)', av: 'var(--cyan)', vb: 'var(--aux)', tp: 'var(--aux)' };
  const analog = $derived((app.recording?.channels ?? []).filter(ch => ch in colors));
  const active = $derived(activeGpi(app.edges));
  const idle = $derived([1,2,3,4,5,6].filter(n => !active.includes(n)));
  const lanes = $derived([...analog, ...(app.recording?.channels.includes('gpi') ? [...active.map(n => 'gpi'+n), ...(idle.length ? expandedIdle ? idle.map(n => 'gpi'+n) : ['idle'] : [])] : []), ...(app.recording?.channels.some(ch => ch === 'rx' || ch === 'tx') ? ['rx'] : [])]);
  const analogHeight = $derived(Math.max(110, (availableHeight - 32 - lanes.filter(ch => ch.startsWith('gpi') || ch === 'idle').length * 24 - (lanes.includes('rx') ? 44 : 0)) / Math.max(analog.length, 1)));
  const laneHeight = (ch: string) => ch === 'rx' ? 44 : ch.startsWith('gpi') || ch === 'idle' ? 24 : analogHeight;
  const height = $derived(32 + lanes.reduce((sum, ch) => sum + laneHeight(ch), 0));
  const topOf = (ch: string) => 32 + lanes.slice(0, lanes.indexOf(ch)).reduce((sum, lane) => sum + laneHeight(lane), 0);
  const x = (t: number) => (t - app.viewRange[0]) / (app.viewRange[1] - app.viewRange[0]) * width;
  const time = (px: number) => app.viewRange[0] + px / width * (app.viewRange[1] - app.viewRange[0]);
  function uartIndex(t: number) { let lo=0, hi=app.uart.length; while (lo<hi) { const mid=(lo+hi)>>1; if (app.uart[mid].t<t) lo=mid+1; else hi=mid; } return lo; }
  const minimumSpan = () => Math.max(1e-6, 20 * Math.min(sampleSpacing, (app.recording?.duration_s ?? 1) / Math.max(1, ...analog.map(ch => app.recording?.samples[ch] ?? 0))));
  const duration = () => Math.max(app.recording?.duration_s ?? 0, minimumSpan());
  const kind = (ch: string): 'current' | 'voltage' | 'power' => ch === 'mc' || ch === 'ac' ? 'current' : ch === 'mp' ? 'power' : 'voltage';
  const unit = (ch: string) => ch === 'tp' ? '°C' : kind(ch) === 'current' ? 'A' : kind(ch) === 'power' ? 'W' : 'V';
  function reading(value: number, ch: string) { if (ch === 'tp') return Number.isFinite(value) ? value.toFixed(2)+' °C' : '—'; const r = formatReading(value, kind(ch)); return r.number.trim()+' '+r.unit; }
  function axisReading(value: number, step: number, ch: string) {
    if (ch === 'tp') return value.toFixed(Math.max(0, Math.ceil(-Math.log10(Math.abs(step) || 1))))+' °C';
    const r = formatReading(value, kind(ch));
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
    for (const ch of lanes) {
      const top=topOf(ch), bottom=top+laneHeight(ch);
      ctx.strokeStyle=color('--border'); ctx.lineWidth=1; ctx.beginPath(); ctx.moveTo(0,bottom-.5); ctx.lineTo(width,bottom-.5); ctx.stroke();
      if (ch === 'idle') continue;
      if (ch.startsWith('gpi')) {
        const bit=Number(ch.slice(3))-1, low=bottom-5, high=top+4;
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
      if (ch === 'rx') {
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
      const y=(v: number) => bottom-8-(transform(v)-min)/(max-min)*(laneHeight(ch)-16);
      const ticks=niceTicks(min,max,5);
      const tickStep=ticks.length > 1 ? ticks[1]-ticks[0] : max-min;
      const eps=(max-min)*1e-6;
      for (const tick of ticks.filter(t => t >= min-eps && t <= max+eps)) { const yy=bottom-8-(tick-min)/(max-min)*(laneHeight(ch)-16); ctx.strokeStyle=color('--border'); ctx.beginPath(); ctx.moveTo(0,yy); ctx.lineTo(width,yy); ctx.stroke(); ctx.fillStyle=color('--muted-foreground'); ctx.fillText(log ? reading(10**tick,ch) : axisReading(tick,tickStep,ch),4,yy-3); }
      const trace=color(colors[ch].slice(4,-1)); ctx.strokeStyle=trace; ctx.fillStyle=trace;
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
  function down(event: PointerEvent) {
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
    } else { if (lanes.includes('rx') && event.offsetY >= topOf('rx') && event.offsetY <= topOf('rx')+44) { const tx=event.offsetY > topOf('rx')+21; hoveredUart=app.uart.slice(Math.max(0,uartIndex(time(event.offsetX))-8),uartIndex(time(event.offsetX))+2).filter(chunk => (chunk.dir === 'tx') === tx && event.offsetX >= x(chunk.t)-6 && event.offsetX <= x(chunk.t)+Math.max(8,printable(chunk.bytes).length*6+12)).at(-1) ?? null; } else hoveredUart=null; app.focusedUartTime=hoveredUart ? lineStartForChunk(app.uart,app.uart.indexOf(hoveredUart)) : null; draw(); }
  }
  function up() { drag=null; if (app.range && app.range[1]-app.range[0]<1e-6) app.range=null; void app.refreshDetails(); }
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
</script>
<section class="recording-panel">
  <div class="recording-toolbar"><strong>Recording</strong><span>{app.recording?.duration_s.toFixed(2) ?? '0.00'} s · {app.recording?.gaps ?? 0} gaps</span><button onclick={() => { app.follow=!app.follow; schedule(); }} class:active={app.follow}>Follow</button><button onclick={() => selecting=!selecting} class:active={selecting}>Select range</button><button onclick={fit}>Fit</button></div>
  <div class="recording-stage" bind:this={host}>
    <div class="lane-headers"><div class="axis-spacer"></div>{#each lanes as ch}
      {#if ch === 'idle'}<button class="idle-lane" style:height={laneHeight(ch)+'px'} onclick={() => expandedIdle=true}>gpi{idle.length > 1 && idle.every((n,i) => n === idle[0]+i) ? idle[0]+'–'+idle.at(-1) : idle.join(', ')} idle ▾</button>
      {:else}<div class="lane-label" style:height={laneHeight(ch)+'px'}><div class="lane-name"><span class="lane-swatch" style:background={colors[ch] ?? (ch.startsWith('gpi') ? 'var(--digital)' : 'var(--uart)')}></span><strong>{ch}</strong><small>{ch in colors ? unit(ch) : ''}</small></div>
        {#if ch in colors}<div class="lane-value">{currentValue(ch)}</div><div class="lane-mean">mean {meanValue(ch)}</div><div class="lane-actions"><button onclick={() => { locked=locked.includes(ch) ? locked.filter(x => x !== ch) : [...locked,ch]; draw(); }} title="Lock y scale" aria-label={'Lock y scale for '+ch}>{locked.includes(ch) ? '🔒' : '⌁'}</button>{#if ch === 'mc' || ch === 'ac'}<button class:active={logarithmic.includes(ch)} onclick={() => { logarithmic=logarithmic.includes(ch) ? logarithmic.filter(x => x !== ch) : [...logarithmic,ch]; delete scales[ch]; draw(); }} title="Log scale" aria-label={'Log scale for '+ch}>log</button>{/if}</div>{/if}</div>{/if}
    {/each}{#if expandedIdle && idle.length}<button class="idle-collapse" onclick={() => expandedIdle=false}>Collapse idle inputs</button>{/if}</div>
    <div class="plot-surface"><canvas bind:this={canvas} style:width={width+'px'} style:height={height+'px'} onwheel={wheel} onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up} ondblclick={fit} onpointerleave={() => { cursor=null; hoveredUart=null; draw(); }}></canvas>
      {#if hoveredUart}<div class="uart-tooltip" style:left={Math.min(width-180,Math.max(4,x(hoveredUart.t)+8))+'px'} style:top={topOf('rx')+4+'px'}><time>{hoveredUart.t.toFixed(6)} s</time><span>{printable(hoveredUart.bytes)}</span><code>{hex(hoveredUart.bytes)}</code></div>{/if}
    </div>
  </div>
</section>
