# Python package `benchwatt`

`benchwatt` requires Python 3.10 or newer. Install a wheel built with maturin, or use this development loop from the repository root:

```sh
python -m venv .venv
. .venv/bin/activate
pip install maturin pytest numpy
maturin develop -m crates/benchwatt-py/Cargo.toml --features testing
pytest crates/benchwatt-py/tests -q
```

Maturin's `-m` flag names the Cargo manifest, so use `crates/benchwatt-py/Cargo.toml` here; `-m crates/benchwatt-py/pyproject.toml` is rejected by current maturin. The `testing` feature is enabled only for the development build. It adds `benchwatt._testing`, with `recording_from_fixture` (a recording replayed from captured Arc traffic) and `fake_device` (a pretend supply with several outputs), so the tests run without hardware. The tests under `tests/hw` talk to real instruments and are skipped unless `BENCHWATT_HW=1` is set.

## Opening a device

The same `Device` class drives every supported instrument: the Otii Arc (`arc`), GW Instek GPP supplies (`gpp`) and the Nordic PPK2 (`ppk2`).

```python
import benchwatt

print(benchwatt.devices())   # [{"kind": "gpp", "port": "/dev/ttyUSB0"}, ...]

dev = benchwatt.open()             # the only device plugged in
dev = benchwatt.open("gpp")        # the GPP, when several are plugged in
dev = benchwatt.open("gpp@/dev/ttyUSB0")   # a device on a given port
```

`open()` without an argument fails with a `ValueError` when several devices are plugged in. `benchwatt.Arc(port=None)` is short for `open("arc")` or `open("arc@PORT")`. A `Device` is a context manager; leaving the `with` block closes it. Closing the PPK2 also switches its output off.

## What a device describes

| Attribute | What it holds |
|-----------|---------------|
| `info` | `kind`, `model`, `serial`, `firmware`, `hardware` (None when the device does not say), then whatever else the driver reports |
| `channels` | One dict per channel: `id`, `name`, `label`, `unit`, `rate_hz`, `kind` (`Analog`, `Digital`, `Bytes`), `quantity`, `group`, `bits`, `extra`, `set` |
| `outputs` | One dict per supply output: `n`, `label`, `group`, `voltage` and `current_limit` ranges (`{"min", "max"}`, `current_limit` None when it cannot be set), `voltage_steps`, `color` |
| `settings` | The device-specific knobs: `key`, `label`, `type` (`bool`, `choice` with `options`, `number` with `min`, `max`, `unit`), `output`, `readonly` |

`get(key)` reads a setting and `set(key, value)` writes one; the value is a bool, one of the choices as a string, or a number, as `settings` describes it.

| Device | Channels | Settings |
|--------|----------|----------|
| Arc | `mc`, `mv`, `mp` (main output), `ac`, `av`, `vb`, `tp`, ..., `gpi` (digital), `rx`, `tx` (UART) | `current_mode` (`cutoff`, `limit`), `io_voltage`, `5v_pin`, `4wire` |
| GPP | `ch1.v`, `ch1.i`, `ch1.p`, ... up to `ch4.*` | `poll_rate`, `ovp1`, `ovp1_on`, `ocp1`, `ocp1_on`, ... |
| PPK2 | `i`, `v` (the voltage that was set), `d` (8 digital inputs) | `mode` (`source`, `ampere`), `spike_filter` |

## Supply outputs

Outputs are numbered from 1. `set_voltage(volts, out=None)`, `set_current_limit(amps, out=None)`, `set_output(on, out=None)` and `output_state(out=None)` act on output `out`. Without `out` they act on the only output, and raise a `ValueError` naming the outputs when the supply has several. `output_state` returns `enabled`, `voltage` and `current_limit`, with None where the device cannot say: the PPK2 has no current limit, and its voltage is unknown until it has been set.

The properties `output` (read and write), `voltage` and `current_limit` are shorthands for the only output and follow the same rule.

```python
import benchwatt

with benchwatt.open("gpp") as gpp:
    print([o["label"] for o in gpp.outputs])   # ['CH1', 'CH2', ...]
    gpp.set_voltage(5.0, out=2)
    gpp.set_current_limit(0.2, out=2)
    gpp.set_output(True, out=2)
    print(gpp.output_state(2))   # {'enabled': True, 'voltage': 5.0, 'current_limit': 0.2}
    rec = gpp.record(10.0, channels=["ch2.v", "ch2.i"])
    gpp.set_output(False, out=2)

print(rec.stats("ch2.i"))
```

## Recording

Device methods use volts and amps; recordings use seconds and SI sample values. `record(duration, channels=None, digital=False, uart=None)` records the named channels. Without `channels` it records the analog channels that measure the outputs (those with a group): `mc`, `mv`, `mp` on the Arc, `i` and `v` on the PPK2, every `chN.*` on a GPP. `digital=True` adds the digital inputs, and a baud rate in `uart` opens the UART for the recording and adds what it receives. `record` blocks but releases Python's GIL while waiting for device data. Ctrl-C stops capture and returns the samples received so far. `start_recording(...)` and `stop_recording()` do the same around your own code.

```python
import benchwatt

with benchwatt.Arc() as arc:
    print(arc.info)
    arc.set_voltage(3.0)
    arc.set_current_limit(0.1)
    arc.set("current_mode", "cutoff")
    arc.output = True
    try:
        rec = arc.record(10.0, channels=["mc", "mv", "mp"])
    finally:
        arc.output = False

rec.save("run.bwrec")
rec.export("run.csv")  # or run.sr
print(rec.stats("mc"), rec.charge_mah(), rec.energy())
```

A recording can also be loaded offline with `benchwatt.Recording.load("run.bwrec")`. `data(name)` returns native rate NumPy arrays: float64 timestamps and float32 values. `stats`, `data`, `edges` and `bytes` accept optional `t0` and `t1` bounds. `edges()` lists the changes of the digital inputs as `{"t", "mask"}`, and `bytes(name)` the data of a bytes channel, such as the Arc's `rx`, as `(t, bytes)` pairs. `charge_mah()` and `energy()` use the first group of channels in the recording.

## Other device functions

A device that lacks what a method needs raises a `ValueError` saying so, for example `device has no UART`.

- `set_gpo(n, on)` sets digital output `n` (the Arc has two).
- `uart_open(baud)`, `uart_write(data)`, `uart_close()` drive the UART. Bytes written during a recording are logged in it too.
- `monitor()` returns the latest live value of each channel the device reports by itself, by channel name; the digital inputs appear as a mask.
- `alerts()` returns and clears what the device reported on its own, as dicts with `id`, `name`, `title`, `detail` and `severity` (`warning` or `error`). Alerts do not raise exceptions.

## Pytest power regression

This example assumes a connected 100 Ω load and a 1.8 V setpoint. Hardware tests should only be run in an environment with the stated load connected.

```python
import pytest
import benchwatt

def test_100_ohm_current():
    with benchwatt.Arc() as arc:
        arc.set_current_limit(0.1)
        arc.set_voltage(1.8)
        arc.output = True
        try:
            rec = arc.record(2.0, channels=["mc"])
        finally:
            arc.output = False
    assert rec.stats("mc")["mean"] == pytest.approx(0.018, rel=0.05)
```

## Battery life from sleep and active capture

For a device with a 500 mAh usable battery, capture separate sleep and active states. If it is active 5% of the time, estimate hours from the weighted current:

```python
sleep = benchwatt.Recording.load("sleep.bwrec")
active = benchwatt.Recording.load("active.bwrec")
sleep_a = sleep.stats("mc")["mean"]
active_a = active.stats("mc")["mean"]
average_a = 0.95 * sleep_a + 0.05 * active_a
hours = 0.5 / average_a
print(f"Estimated battery life: {hours:.1f} h")
```

This estimate assumes the captured states and duty cycle represent real use, and ignores battery voltage and capacity changes with load.

## Plot a native rate trace

```python
import matplotlib.pyplot as plt
import benchwatt

rec = benchwatt.Recording.load("run.bwrec")
t, current = rec.data("mc", t0=1.0, t1=2.0)
plt.plot(t, current * 1000)
plt.xlabel("Time (s)")
plt.ylabel("Current (mA)")
plt.show()
```
