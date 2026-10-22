#!/usr/bin/env python3
"""Read-only probe of the F75 HE over its 2.4 GHz receiver (0C45:FEFE, 0xFF60, 32-byte reports).

Waits until the keyboard answers 0x10 (it sleeps when idle: tap a key), then pages 0x13, 0x17,
0x14, 0x12 and 0x18 in 24-byte chunks (chunk = frame_len - 8) and prints the tables as JSON.
Only GET commands (0x1X) are ever sent.

usage: f75_rf_probe.py /dev/hidrawN [wait_seconds] > out.json
"""
import json, os, select, sys, time

dev = sys.argv[1]
wait = float(sys.argv[2]) if len(sys.argv) > 2 else 180
fd = os.open(dev, os.O_RDWR)
FRAME, PAGE = 32, 24


def send(req):
    assert req[0] == 0xAA and req[1] & 0xF0 == 0x10, "read-only probe"
    os.write(fd, b"\x00" + req)


def transact(cmd, ln, off, last, tries=3, timeout=0.5):
    req = bytes([0xAA, cmd, ln, off & 0xFF, off >> 8, 0, int(last), 0]).ljust(FRAME, b"\0")
    for attempt in range(tries):
        send(req)
        end = time.time() + timeout
        while time.time() < end:
            if select.select([fd], [], [], 0.05)[0]:
                r = os.read(fd, 64)
                if r[0] == 0x55 and r[1] == cmd and r[3:5] == req[3:5]:
                    return r, attempt
    return None, tries


def log(*a):
    print(*a, file=sys.stderr, flush=True)


start = time.time()
while True:
    r, _ = transact(0x10, PAGE, 0, False, tries=1)
    if r:
        break
    if time.time() - start > wait:
        sys.exit("keyboard never answered (asleep?)")
    time.sleep(0.5)
log(f"awake after {time.time() - start:.1f}s: {r.hex(' ')}")

out = {"info": r.hex(), "retries": {}, "ms": {}}
for name, cmd, size in [("light", 0x13, 0x10), ("act", 0x17, 1008), ("leds", 0x14, 504),
                        ("keys", 0x12, 512), ("dks", 0x18, 1024)]:
    t, retries, t0 = bytearray(size), 0, time.time()
    for off in range(0, size, PAGE):
        ln = min(PAGE, size - off)
        r, a = transact(cmd, ln, off, off + ln >= size)
        if r is None:
            log(f"{name}: no reply at offset {off:#x}")
            break
        retries += a
        t[off:off + ln] = r[8:8 + min(r[2], ln)]
    else:
        out[name] = t.hex()
    out["retries"][name] = retries
    out["ms"][name] = round((time.time() - t0) * 1000)
    log(f"{name}: {'ok' if name in out else 'FAILED'}, {retries} retries, {out['ms'][name]} ms")
print(json.dumps(out, indent=1))
