import os
import pytest
import openotii

pytestmark = pytest.mark.skipif(os.getenv("OPENOTII_HW") != "1", reason="requires OPENOTII_HW=1")


def test_100_ohm_load():
    with openotii.Arc() as arc:
        arc.set_current_limit(0.1)
        arc.set_voltage(1.8)
        arc.output = True
        try:
            rec = arc.record(2.0, channels=["mc"])
            assert rec.stats("mc")["mean"] == pytest.approx(0.018, rel=0.05)
        finally:
            arc.output = False


def test_uart_loopback():
    with openotii.Arc() as arc:
        arc.uart_open(115200)
        try:
            arc.start_recording(["rx"])
            arc.uart_write(b"openotii-loopback\n")
            import time
            time.sleep(0.5)
            rec = arc.stop_recording()
            assert b"openotii-loopback\n" in b"".join(data for _, data in rec.uart())
        finally:
            arc.uart_close()


def test_gpo1_to_gpi1():
    with openotii.Arc() as arc:
        arc.start_recording([], gpi=True)
        arc.set_gpo(1, True)
        import time
        time.sleep(0.2)
        arc.set_gpo(1, False)
        time.sleep(0.2)
        rec = arc.stop_recording()
        assert any(edge["mask"] & 1 for edge in rec.edges())
