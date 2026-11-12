<script lang="ts">
  import { tick } from 'svelte';
  import { app } from '$lib/stores.svelte';
  import { linesFromChunks, filterLines, linesInRange, formatUartTime, type UartLine } from '$lib/uart';
  import { Button } from '$lib/components/ui/button';
  import SerialSource from './SerialSource.svelte';
  import { ContextMenu, ContextMenuTrigger, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuShortcut } from '$lib/components/ui/context-menu';
  let query = $state(''), showTx = $state(true), showTimes = $state(true), hex = $state(false);
  let ending = $state('LF'), outgoing = $state(''), history: string[] = [], historyIndex = -1;
  let cutoff = $state(-1), scroll = $state(0), viewport = $state(180), scroller: HTMLDivElement;
  let pinned = $state(true), anchor = $state<number | null>(null);
  let contextLine = $state<UartLine | null>(null);
  const rowHeight = 24;
  const all = $derived(linesFromChunks(app.uart).filter(x => x.t > cutoff));
  const filtered = $derived(filterLines(all,query,showTx));
  const inRange = $derived(linesInRange(filtered,app.range));
  const first = $derived(Math.max(0,Math.floor(scroll/rowHeight)-8));
  const visible = $derived(filtered.slice(first,first+Math.ceil(viewport/rowHeight)+16));
  function asText(line: UartLine) { return hex ? line.bytes.map(x => x.toString(16).padStart(2,'0').toUpperCase()).join(' ') : line.text; }
  function segments(value: string): { text: string; match: boolean }[] {
    if (!query) return [{ text: value, match: false }];
    const lower=value.toLocaleLowerCase(), needle=query.toLocaleLowerCase(), parts: { text: string; match: boolean }[]=[];
    let at=0, found=lower.indexOf(needle);
    while (found >= 0) { if (found > at) parts.push({ text: value.slice(at,found), match: false }); parts.push({ text: value.slice(found,found+needle.length), match: true }); at=found+needle.length; found=lower.indexOf(needle,at); }
    if (at < value.length) parts.push({ text: value.slice(at), match: false });
    return parts;
  }
  function highlighted(line: UartLine) { return app.range ? inRange.includes(line) : app.focusedUartTime === line.t; }
  function scrollTo(t: number) { const index=filtered.findIndex(x => x.t === t); if (index >= 0 && scroller) scroller.scrollTop=Math.max(0,index*rowHeight-viewport/2); }
  function select(line: UartLine, shift: boolean) {
    if (shift && anchor !== null) { app.range=[Math.min(anchor,line.t),Math.max(anchor,line.t)+.001]; void app.refreshDetails(); }
    else { anchor=line.t; app.range=null; app.centerUart(line.t); }
  }
  function clearView() { cutoff=app.recording?.duration_s ?? 0; }
  function contextLines() { return contextLine ? highlighted(contextLine) && app.range ? inRange : [contextLine] : []; }
  function copyLines(lines: UartLine[], asHex = false) {
    if (!lines.length) return;
    void navigator.clipboard.writeText(lines.map(x => `${showTimes ? formatUartTime(x.t,app.viewRange[1]-app.viewRange[0])+'\t' : ''}${x.dir === 'tx' ? '→ ' : ''}${asHex ? x.bytes.map(byte => byte.toString(16).padStart(2,'0').toUpperCase()).join(' ') : asText(x)}`).join('\n'));
  }
  async function send() {
    if (!outgoing || !app.textOpen) return;
    const raw=outgoing, suffix=ending === 'LF' ? '\n' : ending === 'CRLF' ? '\r\n' : '';
    try { await app.sendText(raw+suffix); history.push(raw); historyIndex=history.length; outgoing=''; await app.refreshDetails(); pinned=true; }
    catch (cause) { app.report(cause); }
  }
  function sendKey(event: KeyboardEvent) {
    if (event.key === 'ArrowUp') { event.preventDefault(); historyIndex=Math.max(0,historyIndex-1); outgoing=history[historyIndex] ?? outgoing; }
    else if (event.key === 'ArrowDown') { event.preventDefault(); historyIndex=Math.min(history.length,historyIndex+1); outgoing=history[historyIndex] ?? ''; }
  }
  function onScroll() { if (!scroller) return; scroll=scroller.scrollTop; pinned=scroller.scrollTop+scroller.clientHeight >= scroller.scrollHeight-32; }
  function copy(event: KeyboardEvent) {
    if (!(event.ctrlKey && event.key.toLowerCase() === 'c') || (event.target as HTMLElement).closest('input, textarea, select')) return;
    const lines=app.range ? inRange : filtered.filter(x => x.t === app.focusedUartTime);
    if (!lines.length) return;
    event.preventDefault(); copyLines(lines);
  }
  $effect(() => { const count=filtered.length; if (pinned && scroller) { tick().then(() => { if (scroller) scroller.scrollTop=scroller.scrollHeight; }); } void count; });
  $effect(() => { const t=app.focusedUartTime; if (t !== null && scroller) tick().then(() => scrollTo(t)); });
  $effect(() => { const range=app.range; if (range && scroller) tick().then(() => scrollTo(range[0])); });
</script>
<div class="uart-console" onkeydown={copy} role="listbox" tabindex="0" aria-label="UART console">
  <div class="uart-console-toolbar">
    <SerialSource/>
    <button class:active={!hex} onclick={() => hex=false}>ASCII</button><button class:active={hex} onclick={() => hex=true}>Hex</button>
    <label><input type="checkbox" bind:checked={showTx}/> Show TX</label><label><input type="checkbox" bind:checked={showTimes}/> Timestamps</label>
    <input class="uart-filter" placeholder="Filter lines…" aria-label="Filter UART lines" bind:value={query}/>
    <button onclick={clearView}>Clear view</button><span class="uart-count">{filtered.length} lines</span>
  </div>
  <div class="uart-viewport" bind:this={scroller} bind:clientHeight={viewport} onscroll={onScroll}>
    {#if query && filtered.length === 0 && all.length > 0}<p class="empty-state">No lines match “{query}”</p>
    {:else if filtered.length > 0}<!-- lines below -->
    {:else if !app.selected}<p class="empty-state">No text in this recording</p>
    {:else if !app.textSource}<div class="serial-empty"><strong>See what your target prints next to its current</strong><p>Choose a serial port of this PC under Port. What arrives on it is recorded on the same time axis as the measurement, and you can type back.</p></div>
    {:else if !app.textOpen}<p class="empty-state">Open the port to record what arrives on it</p>
    {:else if !app.live}<p class="empty-state">Text is recorded while a recording runs (press R)</p>{/if}
    <div style:height={filtered.length*rowHeight+'px'} class="uart-virtual-space">
      {#each visible as line, i (line.id)}
        <ContextMenu><ContextMenuTrigger class={'uart-line'+(line.dir === 'tx' ? ' tx' : '')+(highlighted(line) ? ' selected' : '')+(app.focusedUartTime === line.t ? ' hovered' : '')} style={'top:'+((first+i)*rowHeight)+'px'} onclick={e => select(line,e.shiftKey)} oncontextmenu={() => contextLine=line} onmouseenter={() => app.hoverMarker=line.t} onmouseleave={() => app.hoverMarker=null}>
          {#if showTimes}<time>{formatUartTime(line.t,app.viewRange[1]-app.viewRange[0])}</time>{/if}<span class="uart-direction">{line.dir === 'tx' ? '→' : ' '}</span><code>{#each segments(asText(line)) as part}{#if part.match}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}</code>
        </ContextMenuTrigger><ContextMenuContent class="plot-context-menu"><ContextMenuItem onclick={() => copyLines(contextLines())}>Copy line(s)<ContextMenuShortcut>Ctrl+C</ContextMenuShortcut></ContextMenuItem><ContextMenuItem onclick={() => copyLines(contextLines(),true)}>Copy as hex</ContextMenuItem><ContextMenuItem onclick={() => select(line,false)}>Show in plot</ContextMenuItem><ContextMenuItem disabled={anchor === null} onclick={() => select(line,true)}>Select range to here<ContextMenuShortcut>Shift+Click</ContextMenuShortcut></ContextMenuItem><ContextMenuItem onclick={() => query=line.text}>Filter by this text</ContextMenuItem><ContextMenuSeparator/><ContextMenuItem onclick={clearView}>Clear view</ContextMenuItem></ContextMenuContent></ContextMenu>
      {/each}
    </div>
  </div>
  {#if !pinned}<button class="uart-jump" onclick={() => { scroller.scrollTop=scroller.scrollHeight; pinned=true; }}>Jump to latest</button>{/if}
  {#if app.textSource}<form class="uart-send" onsubmit={e => { e.preventDefault(); void send(); }}><input aria-label="UART text to send" placeholder="Send text…" bind:value={outgoing} disabled={!app.textOpen} onkeydown={sendKey}/><select aria-label="Line ending" bind:value={ending}><option>None</option><option>LF</option><option>CRLF</option></select><Button type="submit" size="sm" disabled={!app.textOpen || !outgoing}>Send</Button></form>{/if}
</div>
