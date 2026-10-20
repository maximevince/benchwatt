<script lang="ts">
  import { onMount } from 'svelte';
  import { backend, type View } from '$lib/backend';
  import { app } from '$lib/stores.svelte';
  let host: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let width = $state(700), height = $state(400);
  let follow = $state(true), selecting = $state(false), cursor = $state<number | null>(null);
  let locked = $state<string[]>([]);
  let scales: Record<string, [number, number]> = {};
  const views = new Map<string, View>();
  let previousDuration = 0;
  const inflight = new Set<string>();
  let queued = false, drag: { x: number; range: [number, number]; shift: boolean } | null = null;
  const colors: Record<string, string> = { mc: 'var(--current)', mv: 'var(--voltage)', mp: 'var(--power)', ac: 'var(--teal)', av: 'var(--cyan)', vb: 'var(--aux)', tp: 'var(--aux)' };
  let lanes = $derived((app.recording?.channels ?? []).flatMap(ch => ch === 'gpi' ? ['gpi1','gpi2','gpi3','gpi4','gpi5','gpi6'] : [ch]));
  let laneHeight = $derived(Math.max(48, (height - 32) / Math.max(lanes.length, 1)));
  const x = (t: number) => (t - app.viewRange[0]) / (app.viewRange[1] - app.viewRange[0]) * width;
  const time = (px: number) => app.viewRange[0] + px / width * (app.viewRange[1] - app.viewRange[0]);
  function currentValue(ch: string) {
    const view = views.get(ch);
    const at = cursor;
    if (!view || at == null || !view.t.length) return '';
    const n = view.t.findIndex(t => t >= at);
    const i = n < 0 ? view.t.length - 1 : n;
    if (view.kind === 'raw') return Number.isFinite(view.v[i]) ? view.v[i].toPrecision(4) : '—';
    return Number.isFinite(view.min[i]) ? view.min[i].toPrecision(3) + '–' + view.max[i].toPrecision(3) : '—';
  }
  function formatTime(t: number, span: number) {
    if (span < 0.01) return (t * 1e6).toFixed(0) + ' µs';
    if (span < 1) return (t * 1e3).toFixed(1) + ' ms';
    if (span < 120) return t.toFixed(2) + ' s';
    return (t / 60).toFixed(2) + ' min';
  }
  function schedule() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; void fetchViews(); draw(); });
  }
  async function fetchViews() {
    const [t0, t1] = app.viewRange;
    for (const ch of lanes) {
      if (!(ch in colors) || inflight.has(ch)) continue;
      inflight.add(ch);
      void backend.getView(ch, t0, t1, Math.max(1, Math.floor(width * devicePixelRatio / 2))).then(view => {
        views.set(ch, view); draw();
      }).catch(cause => app.report(cause)).finally(() => {
        inflight.delete(ch);
        if (app.viewRange[0] !== t0 || app.viewRange[1] !== t1) schedule();
      });
    }
    void app.refreshDetails();
  }
  function draw() {
    if (!canvas || width < 1 || height < 1) return;
    const ratio = devicePixelRatio || 1;
    canvas.width = Math.ceil(width * ratio); canvas.height = Math.ceil(height * ratio);
    const ctx = canvas.getContext('2d'); if (!ctx) return;
    ctx.scale(ratio, ratio);
    const css = getComputedStyle(canvas);
    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = css.getPropertyValue('--muted-foreground'); ctx.font = '10px Geist Mono';
    const span = app.viewRange[1] - app.viewRange[0];
    for (let i = 0; i <= 5; i++) {
      const at = width * i / 5;
      ctx.fillText(formatTime(time(at), span), Math.min(at + 4, width - 65), 16);
      ctx.strokeStyle = css.getPropertyValue('--border'); ctx.beginPath(); ctx.moveTo(at, 24); ctx.lineTo(at, height); ctx.stroke();
    }
    lanes.forEach((ch, index) => {
      const top = 32 + index * laneHeight, bottom = top + laneHeight;
      ctx.strokeStyle = css.getPropertyValue('--border'); ctx.beginPath(); ctx.moveTo(0, bottom); ctx.lineTo(width, bottom); ctx.stroke();
      if (ch.startsWith('gpi')) {
        const bit = Number(ch.slice(3)) - 1;
        {
          const y0 = top, baseline = y0 + laneHeight / 4;
          ctx.strokeStyle = css.getPropertyValue('--digital'); ctx.beginPath();
          let mask = 0, lastX = 0;
          for (const edge of app.edges) {
            const xx = x(edge.t), yy = y0 + ((mask >> bit) & 1 ? 4 : laneHeight - 6);
            ctx.moveTo(lastX, yy); ctx.lineTo(xx, yy); ctx.lineTo(xx, y0 + ((edge.mask >> bit) & 1 ? 4 : laneHeight - 6));
            lastX = xx; mask = edge.mask;
          }
          ctx.lineTo(width, y0 + ((mask >> bit) & 1 ? 4 : laneHeight - 6)); ctx.stroke();
          ctx.fillStyle = css.getPropertyValue('--muted-foreground'); ctx.fillText(String(bit + 1), 4, baseline);
        }
        return;
      }
      if (ch === 'rx') {
        for (const chunk of app.uart) {
          if (chunk.t < app.viewRange[0] || chunk.t > app.viewRange[1]) continue;
          const xx = x(chunk.t);
          ctx.strokeStyle = css.getPropertyValue('--uart'); ctx.beginPath(); ctx.moveTo(xx, top + 4); ctx.lineTo(xx, bottom - 4); ctx.stroke();
          if (span < 2) {
            ctx.fillStyle = css.getPropertyValue('--uart');
            ctx.fillText(new TextDecoder().decode(new Uint8Array(chunk.bytes)).replaceAll('\n', '↵'), xx + 3, top + 20);
          }
        }
        return;
      }
      const view = views.get(ch); if (!view || !view.t.length) return;
      const values = (view.kind === 'raw' ? view.v : [...view.min, ...view.max]).filter(Number.isFinite);
      if (!values.length) return;
      let min = Math.min(...values), max = Math.max(...values);
      if (locked.includes(ch) && scales[ch]) [min, max] = scales[ch];
      else { const pad = Math.max((max - min) * 0.08, 1e-9); min -= pad; max += pad; scales[ch] = [min, max]; }
      const y = (v: number) => bottom - 5 - (v - min) / (max - min) * (laneHeight - 10);
      const traceColor = css.getPropertyValue(colors[ch].slice(4, -1));
      ctx.strokeStyle = traceColor; ctx.fillStyle = traceColor;
      if (view.kind === 'raw') {
        let step = Infinity;
        for (let i = 1; i < view.t.length; i++) step = Math.min(step, view.t[i] - view.t[i-1]);
        ctx.beginPath();
        view.t.forEach((t, i) => {
          if (!Number.isFinite(view.v[i])) { ctx.stroke(); ctx.beginPath(); return; }
          if (i === 0 || t - view.t[i-1] > step * 1.5) ctx.moveTo(x(t), y(view.v[i]));
          else ctx.lineTo(x(t), y(view.v[i]));
        });
        ctx.stroke();
      } else {
        ctx.globalAlpha = 0.32;
        for (let i = 0; i < view.t.length; i++) if (Number.isFinite(view.min[i])) ctx.fillRect(x(view.t[i]), y(view.max[i]), Math.max(1, Math.min(3, width / view.t.length)), Math.max(1, y(view.min[i]) - y(view.max[i])));
        ctx.globalAlpha = 1;
      }
    });
    if (cursor != null) { ctx.strokeStyle = css.getPropertyValue('--foreground'); ctx.beginPath(); ctx.moveTo(x(cursor), 24); ctx.lineTo(x(cursor), height); ctx.stroke(); }
    if (app.range) { const a = x(app.range[0]), b = x(app.range[1]); ctx.fillStyle = 'rgba(56,189,248,.13)'; ctx.fillRect(a, 24, b-a, height-24); }
  }
  function wheel(event: WheelEvent) {
    event.preventDefault(); follow = false;
    const anchor = time(event.offsetX), factor = Math.exp(event.deltaY * 0.001);
    const [a,b] = app.viewRange;
    const left = Math.max(0, anchor + (a-anchor)*factor);
    const right = Math.max(left + 0.00001, anchor + (b-anchor)*factor);
    app.viewRange = [left, right]; schedule();
  }
  function down(event: PointerEvent) { canvas.setPointerCapture(event.pointerId); drag = { x: event.offsetX, range: [...app.viewRange], shift: event.shiftKey || selecting }; if (drag.shift) { follow = false; app.range = [time(event.offsetX), time(event.offsetX)]; } }
  function move(event: PointerEvent) {
    cursor = time(event.offsetX);
    if (drag) {
      if (drag.shift) app.range = [Math.min(time(drag.x), time(event.offsetX)), Math.max(time(drag.x), time(event.offsetX))];
      else {
        follow = false;
        const delta = (event.offsetX - drag.x) / width * (drag.range[1]-drag.range[0]);
        const lo = Math.max(0, drag.range[0]-delta);
        app.viewRange = [lo, lo + drag.range[1]-drag.range[0]];
      }
      schedule();
    } else draw();
  }
  function up() { drag = null; if (app.range && app.range[1]-app.range[0] < 0.000001) app.range = null; void app.refreshDetails(); }
  function fit() { follow = false; app.viewRange = [0, Math.max(app.recording?.duration_s ?? 1, 0.001)]; schedule(); }
  onMount(() => {
    const observer = new ResizeObserver(() => { width = Math.max(1, host.clientWidth - 156); height = Math.max(1, host.clientHeight); schedule(); });
    observer.observe(host); return () => observer.disconnect();
  });
  $effect(() => { const duration = app.recording?.duration_s ?? 0; app.viewRange; app.theme; if (duration < previousDuration) views.clear(); previousDuration = duration; if (follow && app.recording?.active) { const end = app.recording.duration_s; const span = app.viewRange[1]-app.viewRange[0]; if (end > span) app.viewRange = [end-span,end]; } schedule(); });
</script>
<section class="recording-panel">
  <div class="recording-toolbar"><strong>Recording</strong><span>{app.recording?.duration_s.toFixed(2) ?? '0.00'} s · {app.recording?.gaps ?? 0} gaps</span><button onclick={() => { follow = !follow; schedule(); }} class:active={follow}>Follow</button><button onclick={() => selecting = !selecting} class:active={selecting}>Select range</button><button onclick={fit}>Fit</button></div>
  <div class="recording-stage" bind:this={host}>
    <div class="lane-headers"><div class="axis-spacer"></div>{#each lanes as ch}<div class="lane-label" style:height={laneHeight + 'px'}><span class="lane-swatch" style:background={colors[ch] ?? (ch.startsWith('gpi') ? 'var(--digital)' : 'var(--uart)')}></span><strong>{ch}</strong>{#if ch in colors}<span class="lane-current">{currentValue(ch)}</span><button onclick={() => locked = locked.includes(ch) ? locked.filter(x => x !== ch) : [...locked,ch]} title="Lock y scale">{locked.includes(ch) ? '🔒' : '⌁'}</button>{/if}</div>{/each}</div>
    <canvas bind:this={canvas} style:width={width + 'px'} style:height={height + 'px'} onwheel={wheel} onpointerdown={down} onpointermove={move} onpointerup={up} ondblclick={fit} onpointerleave={() => { cursor = null; draw(); }}></canvas>
  </div>
</section>
