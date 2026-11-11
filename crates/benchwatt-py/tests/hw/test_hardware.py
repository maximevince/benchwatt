import os
import time
import pytest
import benchwatt

pytestmark = pytest.mark.skipif(os.getenv("BENCHWATT_HW") != "1", reason="requires BENCHWATT_HW=1")


def need(kind):
    if kind not in {d["kind"] for d in benchwatt.devices()}:
        pytest.skip(f"no {kind} plugged in")


def test_100_ohm_load():
    need("arc")
    with benchwatt.Arc() as arc:
        arc.set_current_limit(0.1)
        arc.set_voltage(1.8)
        arc.output = True
        try:
            rec = arc.record(2.0, channels=["mc"])
            assert rec.stats("mc")["mean"] == pytest.approx(0.018, rel=0.05)
        finally:
            arc.output = False


def test_uart_loopback():
    need("arc")
    with benchwatt.Arc() as arc:
        arc.uart_open(115200)
        try:
            arc.start_recording(["rx"])
            arc.uart_write(b"benchwatt-loopback\n")
            time.sleep(0.5)
            rec = arc.stop_recording()
            assert b"benchwatt-loopback\n" in b"".join(data for _, data in rec.bytes("rx"))
        finally:
            arc.uart_close()


def test_gpo1_to_gpi1():
    need("arc")
    with benchwatt.Arc() as arc:
        arc.start_recording([], digital=True)
        arc.set_gpo(1, True)
        time.sleep(0.2)
        arc.set_gpo(1, False)
        time.sleep(0.2)
        rec = arc.stop_recording()
        assert any(edge["mask"] & 1 for edge in rec.edges())


def test_gpp_setpoints_per_output():
    need("gpp")
    with benchwatt.open("gpp") as gpp:
        assert len(gpp.outputs) > 1
        with pytest.raises(ValueError, match="pick one"):
            gpp.set_voltage(1.0)
        gpp.set_output(False, out=2)
        gpp.set_voltage(1.25, out=2)
        assert gpp.output_state(2)["voltage"] == pytest.approx(1.25)


def test_ppk2_voltage_known_once_set():
    need("ppk2")
    with benchwatt.open("ppk2") as ppk2:
        assert ppk2.current_limit is None
        ppk2.output = False
        ppk2.set_voltage(1.8)
        assert ppk2.voltage == pytest.approx(1.8)
