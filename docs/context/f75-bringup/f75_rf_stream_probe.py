#!/usr/bin/env python3
"""Does the F75 HE's 2.4 GHz receiver carry the 0x32 live per-key stream? (32-byte reports, 24-byte chunks)

The MINI 60 HE PRO's bridge drops 0x32; this checks the F75's own receiver. Waits for the keyboard
to wake (tap a key), saves the 0x13 lighting block, enters custom mode 20 (0x23), then:
  A. solid red, 3 frames, waiting up to 0.3 s for a 0x55 per chunk (logs acks + time per frame)
  B. green chase over key indices for 3 s, no waiting (max send rate, replies drained)
  C. blue chase for 3 s, waiting for each chunk's ack (if A saw acks)
and in a finally sends 0x36 and restores the saved lighting block, verified by read-back.
0x32/0x36 are live only (nothing goes to flash). Only 0x1X reads, 0x23, 0x32 and 0x36 can be sent.

Watch for: red, then a green dot running across the keys, then blue, then your own lighting back.

usage: f75_rf_stream_probe.py /dev/hidrawN [wait_seconds]   (the dongle's interface 3)
"""
import os, select, sys, time

dev = sys.argv[1]
wait = float(sys.argv[2]) if len(sys.argv) > 2 else 180
fd = os.open(dev, os.O_RDWR)
FRAME, PAGE = 32, 24
ALLOWED_WRITES = {0x23, 0x32, 0x36}
SLOTS, KEYS = 126, 80


def log(*a):
    print(*a, flush=True)


def req(cmd, ln, off, last, payload=b""):
    assert cmd & 0xF0 == 0x10 or cmd in ALLOWED_WRITES, f"blocked {cmd:#x}"
    r = (bytes([0xAA, cmd, ln, off & 0xFF, off >> 8, 0, int(last), 0]) + payload).ljust(FRAME, b"\0")
    assert len(r) == FRAME
    return r


def wait_reply(cmd, off, timeout):
    end = time.time() + timeout
    while time.time() < end:
        if select.select([fd], [], [], 0.02)[0]:
            r = os.read(fd, 64)
            if r[0] == 0x55 and r[1] == cmd and r[3] | r[4] << 8 == off:
                return r
    return None


def transact(cmd, ln, off, last, payload=b"", tries=3, timeout=0.5):
    q = req(cmd, ln, off, last, payload)
    for _ in range(tries):
        os.write(fd, b"\x00" + q)
        r = wait_reply(cmd, off, timeout)
        if r:
            return r
    raise TimeoutError(f"no reply to {cmd:#x} at offset {off:#x}")


def drain():
    n = 0
    while select.select([fd], [], [], 0.0)[0]:
        os.read(fd, 64)
        n += 1
    return n


def read(cmd, size):
    drain()
    t = bytearray(size)
    for off in range(0, size, PAGE):
        ln = min(PAGE, size - off)
        r = transact(cmd, ln, off, off + ln >= size)
        t[off:off + ln] = r[8:8 + min(r[2], ln)]
    return bytes(t)


def write_light(block):
    b = bytearray(block[:16].ljust(16, b"\0"))
    b[11:14] = b"\0\0\0"
    b[14:16] = b"\xAA\x55"
    drain()
    transact(0x23, 16, 0, True, bytes(b))
    time.sleep(0.05)


def table(colors):
    return b"".join(bytes([i, *colors.get(i, (0, 0, 0))]) for i in range(SLOTS))


def stream(t, ack_timeout=None):
    """One 0x32 frame; returns the number of chunks acked (None = not waited for)."""
    acked = 0 if ack_timeout else None
    for off in range(0, len(t), PAGE):
        ln = min(PAGE, len(t) - off)
        os.write(fd, b"\x00" + req(0x32, ln, off, off + ln >= len(t), t[off:off + ln]))
        if ack_timeout:
            acked += wait_reply(0x32, off, ack_timeout) is not None
    return acked


def chase(rgb, seconds, ack_timeout=None):
    n, acks, t0 = 0, 0, time.time()
    while time.time() - t0 < seconds:
        a = stream(table({n % KEYS: rgb}), ack_timeout)
        acks += a or 0
        replies = drain()
        n += 1
    dt = time.time() - t0
    return n, n / dt, acks, replies


start = time.time()
while True:
    try:
        transact(0x10, PAGE, 0, False, tries=1)
        break
    except TimeoutError:
        if time.time() - start > wait:
            sys.exit("keyboard never answered (asleep?)")
        time.sleep(0.5)
log(f"awake after {time.time() - start:.1f}s")

chunks = -(-SLOTS * 4 // PAGE)
orig = read(0x13, 16)
log("light before:", orig.hex(" "))
try:
    custom = bytearray(orig)
    custom[0], custom[1:5], custom[8], custom[9], custom[10] = 20, b"\x90\xEE\x90\xFF", 1, 5, 5
    write_light(custom)
    log("mode now    :", read(0x13, 16).hex(" "))

    for i in range(3):
        t0 = time.time()
        a = stream(table({k: (255, 0, 0) for k in range(SLOTS)}), 0.3)
        log(f"A red frame {i}: {a}/{chunks} chunks acked, {1000 * (time.time() - t0):.0f} ms")
    time.sleep(2)

    n, fps, _, _ = chase((0, 255, 0), 3)
    log(f"B green chase, no waits: {n} frames, {fps:.1f} fps")

    n, fps, acks, _ = chase((0, 0, 255), 3, 0.3)
    log(f"C blue chase, ack-wait: {n} frames, {fps:.1f} fps, {acks}/{n * chunks} chunks acked")
except TimeoutError as e:
    log("TIMEOUT:", e)
finally:
    drain()
    os.write(fd, b"\x00" + req(0x36, 0x18, 0, True))
    log("0x36 ack:", wait_reply(0x36, 0, 0.5) is not None)
    write_light(orig)
    after = read(0x13, 16)
    log("light after :", after.hex(" "), "EXACT" if after[:11] == orig[:11] else "DIFFERS")
