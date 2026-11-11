import os
import pytest
import benchwatt

pytestmark = pytest.mark.skipif(os.getenv("BENCHWATT_HW") != "1", reason="requires BENCHWATT_HW=1")


def test_100_ohm_load():
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
    with benchwatt.Arc() as arc:
        arc.uart_open(115200)
        try:
            arc.start_recording(["rx"])
            arc.uart_write(b"benchwatt-loopback\n")
            import time
            time.sleep(0.5)
            rec = arc.stop_recording()
            assert b"benchwatt-loopback\n" in b"".join(data for _, data in rec.uart())
        finally:
            arc.uart_close()


def test_gpo1_to_gpi1():
    with benchwatt.Arc() as arc:
        arc.start_recording([], gpi=True)
        arc.set_gpo(1, True)
        import time
        time.sleep(0.2)
        arc.set_gpo(1, False)
        time.sleep(0.2)
        rec = arc.stop_recording()
        assert any(edge["mask"] & 1 for edge in rec.edges())
