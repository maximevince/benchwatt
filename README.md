# OpenOtii

An open-source desktop app, command line tool and Python package for the
[Qoitech Otii Arc](https://www.qoitech.com/otii-arc-pro/) power analyzer, on
Linux.

It talks to the Arc directly over USB using the protocol of Qoitech's version 3
firmware. You don't need the Otii desktop app, an account or a license to
supply power, record current and voltage, look at the result, or script
measurements in CI.

This is an independent project. It isn't affiliated with or supported by
Qoitech.

![OpenOtii recording view](docs/screenshot.png)

## What works

- **Power supply**: output on/off, voltage, current limit, and cutoff or
  constant-current limiting. Over-current alerts are shown as they happen.
- **Recording**: main current (4 kHz), voltage and power, plus the ADC,
  VBUS and temperature channels, digital inputs and UART RX/TX. Recordings are
  kept in memory with a min/max pyramid, so zooming from an hour down to single
  samples stays fast.
- **Viewer**: one lane per channel, with a shared time axis. Wheel to zoom,
  drag to pan, shift-drag to select. A selection shows min/avg/max, charge and
  energy right next to it.
- **UART console**: a line-based log of what the device sent and what you
  typed. Clicking a line jumps to that moment in the plot, and selecting a range
  in the plot highlights the matching lines.
- **Expansion port**: GPO1–2, GPI1–2, the 5 V pin and the I/O voltage.
- **Files**: save and open `.otii-rec` recordings, export to CSV or to a
  sigrok session (`.sr`) for PulseView.
- **Automation**: the `otii` command line tool and the `openotii` Python
  package, both described below.

Tested with an Arc (hardware 1.3, firmware 3.1.3). The Otii Ace and older
firmware are not supported. If your Arc still runs firmware 1.x, update it with
the Otii app first.

## Getting started

You need Rust (stable), Node.js 22 or newer, and the
[Tauri prerequisites for Linux](https://v2.tauri.app/start/prerequisites/#linux)
(webkit2gtk and friends).

Your user needs access to the Arc's serial port. On most distributions that
means being in the `uucp` or `dialout` group:

```sh
sudo usermod -aG uucp $USER   # or dialout on Debian/Ubuntu; log in again afterwards
```

Run the app in development mode:

```sh
cd app
npm install
npm run tauri dev
```

It finds the Arc on its own (USB ID `0fce:d1e6`) and reconnects when you plug
it back in. `npm run tauri build` produces a `.deb` and an AppImage under
`target/release/bundle/`.

Without an Arc connected, `npm run dev` serves the interface at
<http://127.0.0.1:1420> with a simulated device. This is handy for working on
the UI.

## Command line

```sh
cargo build --release -p otii-cli
./target/release/otii info
```

Some examples:

```sh
otii power --volt 3.3V --ilim 100mA on
otii measure --ch mc,mv,mp --duration 10s
otii record --ch mc,mv,mp --gpi --uart 115200 --duration 30s -o boot.otii-rec
otii convert boot.otii-rec boot.sr
otii stats boot.otii-rec --from 2s --to 5s
otii power off
```

`mc`, `mv` and `mp` are the Arc's names for main current, voltage and power.
Values take units (`3.3V`, `50uA`, `100ms`), and `--json` turns every command's
output into JSON.

### Power checks in CI

`measure` accepts assertions and exits with status 1 if any of them fails. A
hardware-in-the-loop job can then guard against power regressions with a
couple of lines:

```sh
otii power --volt 3.3V --ilim 100mA on
otii --json measure --ch mc,mv,mp --duration 10s \
  --assert 'mc.mean<50uA' --assert 'mc.max<=200mA' --assert 'charge<0.1mAh' \
  > power.json
status=$?
otii power off
exit $status
```

Exit codes are 0 (pass), 1 (an assertion failed), 2 (device or file error) and
3 (bad arguments). See [docs/cli.md](docs/cli.md) for everything else.

## Python

```python
import openotii

with openotii.Arc() as arc:
    arc.set_voltage(1.8)
    arc.set_current_limit(0.05)
    arc.output = True
    rec = arc.record(5.0, channels=["mc", "mv", "mp"])
    arc.output = False

print(rec.stats("mc"))            # mean, min, max, rms, ...
t, i = rec.data("mc")             # NumPy arrays
rec.save("run.otii-rec")
```

Build it with maturin from `crates/otii-py`. Recordings saved by the app, the
CLI or Python open in any of the three. Details and a pytest example are in
[docs/python.md](docs/python.md).

## Repository layout

| Path | What's in it |
|------|--------------|
| `crates/otii-arc` | Protocol and serial device driver |
| `crates/otii-rec` | Recording engine, `.otii-rec` format, CSV and sigrok export |
| `crates/otii-cli` | The `otii` command line tool |
| `crates/otii-py` | Python bindings (PyO3) |
| `app/` | Desktop app: Tauri 2 backend, Svelte 5 frontend |
| `docs/PROTOCOL.md` | The Arc's v3 protocol as far as we know it |
| `fixtures/` | Captured device traffic used in tests |

## The protocol

Qoitech doesn't document the USB protocol. `docs/PROTOCOL.md` describes what
we found by reading the vendor application and testing on hardware: framing,
commands, properties, the data packet format and a list of quirks. Entries
marked `[V]` have been verified on a real Arc. Corrections are welcome,
especially from people with other hardware or firmware versions.

A few settings the Arc stores in calibration are never written by this code.
Even so, this is unofficial software driving a power supply: double-check
voltage and current limits before you connect something you care about.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd app && npm run check && npx vitest run
```

Hardware tests are skipped unless you opt in. They expect a 100 Ω load on the
main output and the expansion port wired TX→RX and GPO1→GPI1:

```sh
OPENOTII_HW=1 pytest crates/otii-py/tests/hw
cargo run -p otii-rec --example live
```

## License

OpenOtii is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version. See [LICENSE](LICENSE).
