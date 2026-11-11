import numpy as np
import pytest

import benchwatt
from benchwatt._testing import recording_from_fixture
from pathlib import Path

FIXTURE = Path(__file__).resolve().parents[3] / "fixtures" / "session1.hex"


def test_fixture_data_and_stats(tmp_path):
    rec = recording_from_fixture(str(FIXTURE))
    assert rec.duration > 0
    assert {"mc", "mv"} <= set(rec.channels)
    t, v = rec.data("mc")
    tm, vm = rec.data("mv")
    assert t.dtype == np.float64 and v.dtype == np.float32
    assert t.ndim == v.ndim == 1 and len(t) == len(v) == 4 * len(tm)
    assert tm.dtype == np.float64 and vm.dtype == np.float32
    assert np.all(np.diff(t) > 0)
    assert rec.stats("mc")["count"] == len(v)
    assert rec.stats("mc")["min"] <= rec.stats("mc")["mean"] <= rec.stats("mc")["max"]
    assert rec.charge_mah() == pytest.approx(rec.stats("mc")["integral"] / 3.6)
    a, b = rec.data("mc", t0=float(t[5]), t1=float(t[9]))
    assert 0 < len(a) < len(t) and len(a) == len(b)
    assert rec.energy() is not None
    with pytest.raises(ValueError):
        rec.data("bad")
    with pytest.raises(ValueError):
        rec.data("mc", t0=2, t1=1)

    native = tmp_path / "run.otii-rec"
    csv = tmp_path / "run.csv"
    sr = tmp_path / "run.sr"
    rec.save(str(native))
    rec.export(str(csv))
    rec.export(str(sr))
    loaded = benchwatt.Recording.load(str(native))
    lt, lv = loaded.data("mc")
    np.testing.assert_array_equal(lt, t)
    np.testing.assert_array_equal(lv, v)
    assert loaded.stats("mc")["mean"] == pytest.approx(rec.stats("mc")["mean"])
    assert csv.read_text().startswith("time_s,")
    assert sr.stat().st_size > 0
