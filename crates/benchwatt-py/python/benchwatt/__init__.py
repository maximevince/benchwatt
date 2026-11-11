"""Python bindings for Otii Arc recordings and device control."""
from ._benchwatt import Arc, Recording, DeviceError, TimeoutError, AlertError

__all__ = ["Arc", "Recording", "DeviceError", "TimeoutError", "AlertError"]

try:
    from ._benchwatt import _testing
except ImportError:
    pass
else:
    import sys as _sys
    _sys.modules[__name__ + "._testing"] = _testing
