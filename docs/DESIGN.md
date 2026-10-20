# OpenOtii UI design direction

Inspiration: Saleae Logic 2 (calm neutral chrome, the *data* carries the color) and the
shadcn/ui aesthetic (zinc greys, 1px borders, small radii, crisp type). Instrument software:
dense but breathable, every number legible at a glance, nothing decorative that competes with
traces.

## Stack
- **shadcn-svelte** (Svelte 5 / bits-ui) components + **Tailwind CSS v4**, `lucide-svelte` icons.
  Use shadcn primitives: Button, Switch, Input, Select, Tabs, Tooltip, Sonner (toasts),
  Separator, ScrollArea, Badge, Resizable panes.
- Fonts (bundled locally via `@fontsource`, no network at runtime): **Geist** for UI,
  **Geist Mono** for all numbers/units/hex (tabular-nums).
- Plot (Phase 3): **uPlot** on canvas, themed with the tokens below.

## Color
Neutral base, shadcn "zinc". Dark is the default; light theme fully supported (toggle in the
top bar, persisted; default follows the OS).

| token | dark | light |
|---|---|---|
| background | zinc-950 `#09090b` | white |
| panel / card | zinc-900 `#18181b` | zinc-50 |
| border | zinc-800 `#27272a` | zinc-200 |
| muted text | zinc-400 | zinc-500 |
| foreground | zinc-50 | zinc-950 |

Color only where it means something:
- **Channel colors** (traces, swatches, readout accents) — fixed per channel everywhere:
  current `mc` **amber-400 `#fbbf24`**, voltage `mv` **sky-400 `#38bdf8`**, power `mp`
  **violet-400 `#a78bfa`**, ADC `ac/av` **teal/cyan-400**, VBUS/aux voltages **slate-400**,
  GPI/GPO **emerald-400**, UART RX **pink-400**. Light theme uses the -600 shades.
- **Output ON**: emerald (switch + a thin glowing ring on the supply card). OFF is neutral.
- **Recording**: red-500 dot (pulsing) + elapsed time in the top bar.
- **Alerts**: red-500 for faults (overcurrent, overvolt, overtemp), amber for warnings
  (VBUS undervolt). Never use red for anything else.
- No gradients, no shadows beyond shadcn defaults, radius `0.5rem` (cards) / `0.375rem` (inputs).

## Layout (single window, min 1100×700)
```
┌ top bar ─────────────────────────────────────────────────────────────────┐
│ ◉ OpenOtii   [● Arc · fw 3.1.3 · 5120…]      [● REC 00:12]  [☾]  [⋯]    │
├──────────────┬───────────────────────────────────────────────────────────┤
│ SUPPLY       │                                                           │
│  3.2974 V    │                 timeline / plot area                      │
│  18.42 mA    │     (Phase 2: rolling 60 s chart of live monitor V/I/P;   │
│  60.7 mW     │      Phase 3: full recording plot with lanes per channel) │
│ [ OUTPUT ⏻ ] │                                                           │
│ V  [3.300]   │                                                           │
│ I  [3000 mA] │                                                           │
│ mode Cutoff▾ │                                                           │
│──────────────│                                                           │
│ CHANNELS     ├───────────────────────────────────────────────────────────┤
│ ■ mc ■ mv …  │ bottom drawer (tabs): Alerts │ UART │ Selection stats     │
│──────────────│                                                           │
│ EXPANSION    │                                                           │
│ GPO 1 2 3    │                                                           │
│ GPI ●○○○○○   │                                                           │
│──────────────│                                                           │
│ STATUS       │                                                           │
│ VBUS 5.05 V  │                                                           │
│ Temp 23.0 °C │                                                           │
└──────────────┴───────────────────────────────────────────────────────────┘
```
- Left sidebar ~300 px, resizable; sections with small uppercase muted labels
  (`text-xs tracking-wider`), separated by borders, not boxed cards-within-cards.
- Supply readouts are the hero: voltage/current/power at ~28–32 px Geist Mono, unit in muted
  smaller text, a 3 px channel-color bar on the left of each readout. Auto-ranged units
  (nA/µA/mA/A, mW/W) with stable width so digits don't jump.
- Setpoint inputs: compact, unit suffix inside the input, Enter/blur applies; a subtle check
  flash when the device confirms; show the device's limit (e.g. "max 3.76 V") as helper text.
- Bottom drawer resizable/collapsible; Alerts tab shows a badge count.
- Disconnected state: everything stays laid out but dimmed/disabled; centered "Waiting for
  Arc…" overlay in the plot area with a hint about the udev rule.
- Keyboard: `Space` toggles output only with a confirm-less but visible toast; `R` start/stop
  recording (Phase 3). Tooltips show shortcuts.

## Motion
Minimal: 150 ms color/opacity transitions; no layout animation; readouts update at ≤10 Hz with
no tweening (instrument values must be honest).
