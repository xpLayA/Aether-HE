"""Shared F75 HE bring-up helpers (scratchpad, not repo code)."""
import json, os, sys, threading
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "legacy", "python")
sys.path.insert(0, ROOT)
import hid
import protocol_mini60 as pm
from drivers.mini60 import Mini60Driver

VID, PID, UP = 0x38A6, 0x2729, 0xFF68
READ_CMDS = {0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x17}
OUT = os.path.dirname(os.path.abspath(__file__)) + "/../f75"
os.makedirs(OUT, exist_ok=True)


class Dev:
    """Minimal AulaDevice stand-in. allowed = set of vendor cmd bytes this
    session may send; anything else raises BEFORE hitting the wire."""
    def __init__(self, allowed):
        self.allowed = set(allowed)
        self._dev = None
        self._lock = threading.RLock()
        self.sent = []

    def open(self):
        if self._dev is None:
            cands = [d for d in hid.enumerate(VID, PID) if d["usage_page"] == UP]
            if not cands:
                raise IOError(f"no 0x{UP:04X} interface on {VID:04X}:{PID:04X}")
            self._dev = hid.device()
            self._dev.open_path(cands[0]["path"])
        return {"path": "ok"}

    def is_open(self):
        return self._dev is not None

    def close(self):
        if self._dev is not None:
            self._dev.close(); self._dev = None

    def write(self, payload):
        frame = list(payload)[1:]
        cmd = frame[1] if len(frame) > 1 else None
        if frame[0] != 0xAA or cmd not in self.allowed or cmd in pm.DANGEROUS_CMDS:
            raise RuntimeError(f"BLOCKED frame cmd={cmd!r} header={frame[:8]}")
        self.sent.append(cmd)
        n = self._dev.write(bytes(payload))
        if n != len(payload):
            raise IOError(f"short write {n}/{len(payload)}")

    def read(self, length, timeout_ms=100):
        return self._dev.read(length, timeout_ms)


class Profile:
    slug = "aula-f75-he-bringup"
    name = "AULA F75 HE (bring-up)"
    protocol = "protocol_mini60"
    frameLen = 64
    lighting = None   # filled from registry by caller when needed


def driver(allowed, lighting=None):
    p = Profile(); p.lighting = lighting
    d = Dev(allowed); d.open()
    return Mini60Driver(p, d), d


def save(name, obj):
    with open(f"{OUT}/{name}", "w") as f:
        json.dump(obj, f, indent=1)
