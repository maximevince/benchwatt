# Python package `benchwatt`

`benchwatt` requires Python 3.10 or newer. Install a wheel built with maturin, or use this development loop from the repository root:

```sh
python -m venv .venv
. .venv/bin/activate
pip install maturin pytest numpy
maturin develop -m crates/benchwatt-py/Cargo.toml --features testing
pytest crates/benchwatt-py/tests -q
```

Maturin's `-m` flag names the Cargo manifest, so use `crates/benchwatt-py/Cargo.toml` here; `-m crates/benchwatt-py/pyproject.toml` is rejected by current maturin. The `testing` feature is enabled only for the development build.

The development build enables the feature gated fixture helper `benchwatt._testing.recording_from_fixture`. Device methods use volts and amps; recordings use seconds and SI sample values. `record` blocks but releases Python's GIL while waiting for device data. Ctrl-C stops capture and returns the samples received so far. `Arc` can auto-detect its USB serial port or take a path.

```python
import benchwatt

with benchwatt.Arc() as arc:
    print(arc.info)
    arc.set_voltage(3.0)
    arc.set_current_limit(0.1)
    arc.set_current_mode("cutoff")
    arc.output = True
    try:
        rec = arc.record(10.0, channels=["mc", "mv", "mp"])
    finally:
        arc.output = False

rec.save("run.otii-rec")
rec.export("run.csv")  # or run.sr
print(rec.stats("mc"), rec.charge_mah(), rec.energy())
```

A recording can also be loaded offline with `benchwatt.Recording.load("run.otii-rec")`. `data("mc")` returns native rate NumPy arrays: float64 timestamps and float32 values. `stats`, `data`, `edges`, and `uart` accept optional `t0` and `t1` bounds. `alerts()` returns and clears device alerts; alerts do not raise exceptions automatically.

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
sleep = benchwatt.Recording.load("sleep.otii-rec")
active = benchwatt.Recording.load("active.otii-rec")
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

rec = benchwatt.Recording.load("run.otii-rec")
t, current = rec.data("mc", t0=1.0, t1=2.0)
plt.plot(t, current * 1000)
plt.xlabel("Time (s)")
plt.ylabel("Current (mA)")
plt.show()
```
