# Otii Arc — v3 firmware USB protocol (reverse engineered)

Source: `otii_core` from Otii 3.7.6 (`otii::plugin::Arc`, unstripped), verified live
against an Arc (hw 1.3, fw 3.1.3) on 2026-10-20. [V] = verified on hardware,
[S] = read from static analysis only.

USB CDC-ACM, VID 0x0fce PID 0xd1e6. Baud rate irrelevant. Device is silent until spoken to. [V]

## Framing [V]
All little endian.

    u32 magic 0x7FB52CA3 | u16 len | u16 sum16(payload bytes) | payload[len]

Bad checksum → frame dropped silently (no NAK). Max payload 4128 (from hello).

Host → device payload: `u32 txn_id | u32 cmd_id | data`. txn_id increments from 0.

Device → host payload starts with u32 type:

| type        | meaning                |
|-------------|------------------------|
| 0xFF99030E  | command response       |
| 0xFF2A8369  | sample data            |
| 0xFF2FF2CE  | update (props/monitor) |
| 0xFFC593CA  | alert                  |

## Command response [V]
`type | u32 txn_id | i32 status | body`. status 0 = OK, -3 = invalid property.

## Commands
| cmd  | name                 | data                                   |    |
|------|----------------------|----------------------------------------|----|
| 0x0A | ping / time sync     | u64 host time (µs), sent every 1 s. **Required before recording works.** | [V] |
| 0x14 | hello                | — ; resp: u32 proto(3), u32 maxlen(4128), u32 devtype ("Arc " = 0x41726320), u32 featurebits | [V] |
| 0x18 | fw chunk             | u32 seq + ≤4096 B                      | [S] |
| 0x1A | fw finish            | —                                      | [S] |
| 0x64 | get property         | u32 id, u32 0 → value at body          | [V] |
| 0x66 | set property         | u32 id, value → resp body echoes value (i32 props) or empty | [V] |
| 0x68 | commit/persist       | —                                      | [S] |
| 0x6A | calibrate            | —                                      | [S] |
| 0x78 | channel enable       | u32 ch, u32 0/1                        | [V] |
| 0x7C | record start         | —                                      | [V] |
| 0x7E | record stop          | —                                      | [V] |
| 0x82 | channel write (UART tx) | u32 0x19 + bytes                    | [V] |
| 0x8C/0x8E | battery profile start/stop (after SET 0x15 blob) | — | [S] |
| 0x90/0x92 | battery script start/stop (after SET 0x47 blob)  | — | [S] |
| 0x94/0x96 | IV-sim start/stop (after SET 0x49 table)          | — | [S] |

## Properties (get 0x64 / set 0x66)
id modifiers: `|0x400` _min, `|0x800` _max, `|0xC00` _enum (i32[]), `|0x1000` _f (f32 SI), `|0x1400` _ss (f32 step).
If device reports `_f`, host sets as `[id|0x1000, f32]`.

| id   | name            | type                                   |
|------|-----------------|----------------------------------------|
| 0x01 | name            | string ("Arc") [V]                     |
| 0x04 | intres          | i32 mΩ                                 |
| 0x05 | 4wire           | bool                                   |
| 0x06 | mainsrccurrmode | enum 0 cutoff, 1 limit                 |
| 0x07 | forceoff        | int                                    |
| 0x08 | highrange       | bool                                   |
| 0x09 | main            | bool (output on/off) [V read]          |
| 0x0A | pwrreg          | enum 0 voltage, 1 current, 10 inline, 100 off |
| 0x0B | mainvolt        | i32 µV (3300000) [V]                   |
| 0x0C | maxcur          | i32 mA (3000) [V]                      |
| 0x0D | maincur         | i32 µA                                 |
| 0x1E | adcres          | i32 µΩ                                 |
| 0x28 | uart            | enable                                 |
| 0x29 | baudrate        | int                                    |
| 0x32 | gpiocfg         | 3 bits per gpo at shift 0,3,6: 0 = unchanged, 1 = low, 2 = high [V gpo1] |
| 0x33 | expvolt         | i32 µV                                 |
| 0x34 | expoutvolt      | i32 µV                                 |
| 0x80 | hwversion       | string ("1.3") [V]                     |
| 0x81 | fwversion       | string ("3.1.3") [V]                   |
| 0x82 | uid             | string [V]                             |
| 0x8D | chinfo          | 16-byte records, see below [V]         |
| 0x51–0x60, 0xB1, 0xB5–0xBB | calibration tables | don't touch     |

## chinfo (0x8D) [V]
Per channel: `u16 ch | u8 kind (0 samples, 1 bytes, 2 digital) | u8 flags (bit7 f32, bit6 apply scale) | u8 | pad[3] | u32 rate_hz | f32 scale`.
Arc 3.1.3: mc/mp/mc3 4000 Hz f32; others 1000 Hz f32; rx bytes; gpi digital.

## Data packet [V]
`type | u32 ms_counter | blocks...`. One packet per ms. Counter jump > 1 = lost data.
Block: `u16 ch | u16 n | u32 rate_hz | n × (f32 | i32 | bytes)`. Byte blocks (rx) are not padded.
Digital (gpi) blocks: n × u32 bitmask, bits 0..5 = gpi1..6.

## Channels
0 mc (A), 1 mv (V), 2 ac (A), 3 av (V), 4 sn, 5 sp (V), 6 mp (W), 7 ap (W),
0xC–0xE mc1–3, 0x10 vb (VBUS), 0x11 vj (DC jack), 0x12 vpoe, 0x13 vpwr, 0x14 tp (°C),
0x15 tm, 0x16 gpi, 0x17 rx (UART), 0x19 tx, 0x1A–0x1F gpi1–6. min/max decimated: id|0x20 / id|0x30.

## Update packet [V]
`type | u32 0 | records`, record = `u16 rtype | u16 len | u32 id | body`.
- rtype 1: property changed (same decoding as get).
- rtype 2: live monitor, ~5 Hz unsolicited after hello: `ch_id, f32`. 0x16 = gpi mask;
  0x10002 = 4-wire state (0 cal_invalid, 1 disabled, 2 inactive, 3 active);
  0x10000/1/3/4 = battery sim/profile/script.

## Alerts [S]
`type | u32 | records u16 id | u16 len | data`. 1 overtemp, 2 vbus-undervolt, 3 vdc-overvolt,
4 voltcal-fail, 5 main-overvolt, 6 main-overcurr, 7 loadsw-overload, 8 dcdc-overload,
9 prereg-overload, 10 sink-overload, 11 negative-intres, 12 bad-firmware.

## Init sequence (vendor)
hello → GET every property → GET 0x829 (Arc: -3) → GET 0x29 → ping every 1 s.
Recording: 0x78 per channel → 0x7C … 0x7E → 0x78 disable.

## Phase 0 hardware results (2026-10-20, Arc fw 3.1.3, output open)
- SET mainvolt 1.8 V / maxcur 10 mA / main=1 → monitor mv = 1.799 V, no alerts. [V]
- Arc reports `mainvolt_max` (0x80B) = 3.764 V on USB power; `_f/_min/_ss` and `pwrreg_enum` → -3.
  So on Arc, settings are integer-only.
- GPO1→GPI1 loopback: gpiocfg=2 → gpi mask 0x9, gpiocfg=1 → 0x0. Bit 0 = gpi1. Bit 3 also goes
  high with gpo1 high — unexplained (maybe gpi4 or a readback). gpiocfg=0 leaves state unchanged.
- UART TX→RX loopback at 115200: SET baudrate(0x29), SET uart(0x28)=1, 0x78 [0x17,1], 0x82 tx.
  RX bytes only arrive in DATA packets **while recording** (0x7C). Block ch 0x17, n bytes, no padding.
- expvolt (0x33) defaults to 1.9 V = expansion I/O level.
- 100 Ω load (measures 97.7 Ω), 1.8 V, maxcur 50 mA: mc mean 18.396 mA (σ 1 µA, 4 kHz),
  mv 1.798 V. [V]
- `mainsrccurrmode` 0 = cutoff: maxcur 10 mA below load → alert 6 (main-overcurr) and the
  device switches `main` off itself. [V]
- `mainsrccurrmode` 1 = limit: constant current 9.98 mA, mv sags to 0.976 V, no alert. [V]
