import pytest

import benchwatt
from benchwatt._testing import fake_device


def test_descriptions():
    with fake_device(outputs=2) as dev:
        info = dev.info
        assert info["kind"] == "fake" and info["model"] == "Fake supply"
        assert info["serial"] is None and info["outputs"] == 2
        names = [c["name"] for c in dev.channels]
        assert names == ["d", "ch1.v", "ch1.i", "ch2.v", "ch2.i"]
        assert dev.channels[1]["group"] == "ch1" and dev.channels[1]["unit"] == "V"
        out = dev.outputs[1]
        assert out["n"] == 2 and out["label"] == "CH2" and out["color"] == "yellow"
        assert out["voltage"] == {"min": 0.0, "max": 32.0}
        assert out["current_limit"] == {"min": 0.0, "max": 3.2}
        ovp = dev.settings[0]
        assert ovp["key"] == "ovp1" and ovp["type"] == "number" and ovp["output"] == 1


def test_get_set():
    dev = fake_device()
    assert dev.get("ovp1") == 5.0 and dev.get("beep") is True
    dev.set("ovp1", 6.5)
    dev.set("beep", False)
    assert dev.get("ovp1") == 6.5 and dev.get("beep") is False
    with pytest.raises(ValueError, match="unknown setting"):
        dev.get("nope")


def test_several_outputs_need_a_name():
    dev = fake_device(outputs=2)
    for call in (
        lambda: dev.set_voltage(5.0),
        lambda: dev.set_current_limit(0.1),
        lambda: dev.set_output(True),
        lambda: dev.output_state(),
        lambda: dev.voltage,
    ):
        with pytest.raises(ValueError, match=r"1 \(CH1\), 2 \(CH2\)"):
            call()
    with pytest.raises(ValueError, match="pick one"):
        dev.output = True
    assert dev.output_state(out=2) == {"enabled": False, "voltage": None, "current_limit": 1.0}
    dev.set_voltage(5.0, out=2)
    dev.set_current_limit(0.25, out=2)
    dev.set_output(True, out=2)
    assert dev.output_state(2) == {"enabled": True, "voltage": 5.0, "current_limit": 0.25}
    assert dev.output_state(1)["enabled"] is False
    with pytest.raises(ValueError, match="no output 3"):
        dev.set_voltage(1.0, out=3)
    with pytest.raises(ValueError, match="out of range"):
        dev.set_voltage(40.0, out=1)


def test_single_output_is_the_default():
    dev = fake_device(outputs=1)
    assert dev.voltage is None
    dev.set_voltage(3.3)
    dev.output = True
    assert dev.output and dev.voltage == 3.3 and dev.current_limit == 1.0
    dev.set_output(False)
    assert dev.output_state() == {"enabled": False, "voltage": 3.3, "current_limit": 1.0}


def test_missing_capabilities():
    dev = fake_device()
    with pytest.raises(ValueError, match="no digital outputs"):
        dev.set_gpo(1, True)
    for call in (lambda: dev.uart_open(115200), lambda: dev.uart_write(b"x"), dev.uart_close):
        with pytest.raises(ValueError, match="no UART"):
            call()
    with pytest.raises(ValueError, match="no UART"):
        dev.record(0.1, uart=115200)


def test_events():
    dev = fake_device()
    assert dev.monitor() == {"ch1.v": 1.5, "d": 2.0}
    assert dev.alerts() == [{
        "id": 7, "name": "ovp", "title": "Over-voltage",
        "detail": "CH1 went over 5 V", "severity": "error",
    }]
    assert dev.alerts() == []


def test_record_by_channel_name():
    dev = fake_device()
    rec = dev.record(5.0)
    assert rec.channels == ["ch1.v", "ch1.i", "ch2.v", "ch2.i"]
    assert rec.stats("ch2.i")["mean"] == pytest.approx(2.0)
    assert rec.stats("ch1.v")["count"] == 100
    rec = dev.record(5.0, channels=["ch2.v"], digital=True)
    assert rec.channels == ["d", "ch2.v"]
    assert [e["mask"] for e in rec.edges()] == [0, 1, 0, 1]
    with pytest.raises(ValueError, match="unknown channel mc"):
        dev.record(0.1, channels=["mc"])
    dev.start_recording(["ch1.i"])
    with pytest.raises(ValueError, match="already active"):
        dev.start_recording()
    assert dev.stop_recording().channels == ["ch1.i"]


def test_closed():
    dev = fake_device()
    dev.close()
    with pytest.raises(benchwatt.DeviceError, match="closed"):
        dev.info


def test_open_unknown_kind():
    with pytest.raises(ValueError, match="unknown device kind nope"):
        benchwatt.open("nope@/dev/null")
