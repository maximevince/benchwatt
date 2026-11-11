# `benchwatt` command line interface

Build with `cargo build -p benchwatt-cli`; the binary is `target/debug/benchwatt`. `benchwatt devices` lists the supported devices that are plugged in. Live commands use the one that is connected; with several, pick one by kind with `--device arc`. A device that cannot be found by its USB id is named with its port, `--device arc@/dev/ttyACM0`.

`power` sets a supply output: `--volt`, `--ilim`, and `on` or `off`. On a supply with several outputs it needs `--out N`; with a single output that one is used. Without anything to set, `power` lists the outputs with their state and setpoints. Settings that only some devices have, such as the Arc's current mode, go through `get` and `set`. `convert` and `stats` work entirely offline.

## Scripting

Without `--json`, commands print short human-readable summaries (`measure` and `stats` as a table with PASS/FAIL lines). `--json` prints one JSON object per completed command. `monitor` prints JSON lines as events arrive. Errors go to stderr as `{"error":"..."}`. Exit codes are 0 for success, 1 for a failed measurement assertion, 2 for device or file I/O failure, and 3 for invalid arguments.

Numeric arguments accept SI values: `3.3V`, `3300mV`, `50uA` or `50µA`, `10mA`, `0.5`, `100ms`, `2s`, and `1min`. Bare numbers use the command's base unit (V, A, or seconds). Assertion thresholds also accept `mW`, `mWh`, and `mAh`.

```sh
benchwatt --json info
benchwatt power --volt 3.3V --ilim 200mA on
benchwatt set current_mode limit
benchwatt --device gpp power --out 2 --volt 5V --ilim 500mA on
benchwatt --device gpp power
benchwatt power off
benchwatt measure --ch mc,mv,mp --duration 10s --assert 'mc.mean<50uA' --assert 'mc.max<=200mA'
benchwatt record --ch mc,mv,mp --duration 10s -o run.bwrec
benchwatt record --ch mc,mv --duration 10s --gpi --uart 115200 -o run.sr
benchwatt convert run.bwrec run.csv
benchwatt convert run.bwrec run.sr
benchwatt stats run.bwrec --from 1s --to 3s
```

`measure` reports mean, min, max, RMS, peak to peak, and integral for each analog channel, plus charge in mAh and energy in joules/mWh with average power in watts. Each assertion has its measured value and pass status in JSON. Ctrl-C stops `measure` and `record` cleanly; `record` writes the samples received so far.

For a CI power regression check, for example:

```yaml
- name: Check idle current
  run: |
    benchwatt power --volt 3.3V --ilim 100mA on
    benchwatt --json measure --ch mc,mv,mp --duration 10s \
      --assert 'mc.mean<50uA' --assert 'mp.mean<1mW' \
      > power-check.json
```

## Recording formats

`.bwrec` is a deflated ZIP archive with `meta.json` (version 1, timestamp, device and channel details, timing and gaps), raw little endian f32 analog samples in `ch/<name>.f32`, `edges.json`, and `uart.bin` with `uart.json` byte offsets. The loader rebuilds the display pyramid. Raw sample timing is stored per packet so gaps survive a save/load cycle.

`.csv` exports a single wide file at the highest analog sample rate. Lower rate analog channels hold their last sample for each packet; missing data produces empty cells. The library also provides `export_csv_channel` for native rate `time_s,value` output and accepts an optional time range for either CSV exporter.

`.sr` exports a sigrok srzip session at the highest analog sample rate, with analog f32 samples and two logic probes (GPI1 and GPI2) if digital inputs were recorded. Gaps are NaN in analog channels and retain the last logic level. UART bytes are not exported to `.sr`; use `.bwrec` to retain UART data. The srzip layout follows [libsigrok's exporter](https://github.com/sigrokproject/libsigrok/blob/master/src/output/srzip.c).

The existing `get`, `set`, `on`, `off`, `volt`, `ilim`, `monitor`, `uart`, and `gpo` commands remain available. The old `record --secs` form is also accepted.
