"""Python bindings for Benchwatt: bench power instruments and their recordings."""
from ._benchwatt import Device, Recording, Serial, devices, open, DeviceError, TimeoutError, AlertError

__all__ = ["Arc", "Device", "Recording", "Serial", "devices", "open", "DeviceError", "TimeoutError", "AlertError"]


def Arc(port=None):
    """Open an Otii Arc: the one found by USB id, or the one on `port`."""
    return open("arc" if port is None else f"arc@{port}")


try:
    from ._benchwatt import _testing
except ImportError:
    pass
else:
    import sys as _sys
    _sys.modules[__name__ + "._testing"] = _testing
