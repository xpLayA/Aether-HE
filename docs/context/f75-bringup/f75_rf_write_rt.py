#!/usr/bin/env python3
"""Write round-trips on the F75 HE over its 2.4 GHz receiver (32-byte reports, 24-byte chunks).

Waits for the keyboard to wake (tap a key), then, stopping at the first failure:
  1. lighting 0x23: Static red for 2 s, read back, restore the saved block
  2. actuation 0x27: W (index 34) fixed 2.00 mm, read back the whole table, restore
  3. per-key colours 0x24: W -> 01 02 03, read back, restore
  4. DKS 0x28 then key records 0x22: rewrite unchanged, read back
Every write page must be acked with 0x55; every restore is verified byte-exact.
Only 0x1X reads and 0x22/0x23/0x24/0x27/0x28 are allowed; nothing else can be sent.

usage: f75_rf_write_rt.py /dev/hidrawN [wait_seconds]
"""
import os, select, sys, time

dev = sys.argv[1]
wait = float(sys.argv[2]) if len(sys.argv) > 2 else 180
fd = os.open(dev, os.O_RDWR)
FRAME, PAGE = 32, 24
ALLOWED_WRITES = {0x22, 0x23, 0x24, 0x27, 0x28}
W = 34


def log(*a):
    print(*a, flush=True)


def transact(cmd, ln, off, last, payload=b"", tries=3, timeout=0.5):
    assert cmd & 0xF0 == 0x10 or cmd in ALLOWED_WRITES, f"blocked {cmd:#x}"
    req = (bytes([0xAA, cmd, ln, off & 0xFF, off >> 8, 0, int(last), 0]) + payload).ljust(FRAME, b"\0")
    assert len(req) == FRAME
    for attempt in range(tries):
        os.write(fd, b"\x00" + req)
        end = time.time() + timeout
        while time.time() < end:
            if select.select([fd], [], [], 0.05)[0]:
                r = os.read(fd, 64)
                if r[0] == 0x55 and r[1] == cmd and r[3:5] == req[3:5]:
                    return r, attempt
    raise TimeoutError(f"no reply to {cmd:#x} at offset {off:#x}")


def drain():
    while select.select([fd], [], [], 0.02)[0]:
        os.read(fd, 64)


def read(cmd, size):
    drain()
    t = bytearray(size)
    for off in range(0, size, PAGE):
        ln = min(PAGE, size - off)
        r, _ = transact(cmd, ln, off, off + ln >= size)
        t[off:off + ln] = r[8:8 + min(r[2], ln)]
    return bytes(t)


def write(cmd, data):
    drain()
    retries = 0
    for off in range(0, len(data), PAGE):
        ln = min(PAGE, len(data) - off)
        _, a = transact(cmd, ln, off, off + ln >= len(data), data[off:off + ln])
        retries += a
    time.sleep(0.05)
    return retries


def light_frame(block):
    b = bytearray(block[:16].ljust(16, b"\0"))
    b[11:14] = b"\0\0\0"
    b[14:16] = b"\xAA\x55"
    return bytes(b)


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

results = {}
try:
    # 1. lighting
    orig_light = read(0x13, 16)
    log("light before:", orig_light.hex(" "))
    red = bytearray(orig_light)
    red[0], red[1:4], red[4], red[8], red[9] = 1, b"\xFF\0\0", 0xFF, 0, 5
    try:
        r = write(0x23, light_frame(red))
        back = read(0x13, 16)
        log("light red   :", back.hex(" "), f"(retries {r})")
        time.sleep(2)
        ok = back[0] == 1 and back[9] == 5
    finally:
        write(0x23, light_frame(orig_light))
        after = read(0x13, 16)
        log("light after :", after.hex(" "))
    results["lighting"] = ok and after[:11] == orig_light[:11]
    log("LIGHTING", "OK" if results["lighting"] else "FAILED")
    if not results["lighting"]:
        raise SystemExit

    # 2. actuation
    orig_act = read(0x17, 1008)
    new = bytearray(orig_act)
    new[W * 8 + 1] &= 0xFE
    new[W * 8 + 2:W * 8 + 4] = (200).to_bytes(2, "little")
    try:
        r = write(0x27, bytes(new))
        ok = read(0x17, 1008) == bytes(new)
        log("act W=2.00mm read back", "exact" if ok else "DIFFERS", f"(retries {r})")
    finally:
        write(0x27, orig_act)
        restored = read(0x17, 1008) == orig_act
        log("act restore", "EXACT" if restored else "DIFFERS")
    results["actuation"] = ok and restored
    log("ACTUATION", "OK" if results["actuation"] else "FAILED")
    if not results["actuation"]:
        raise SystemExit

    # 3. per-key colours
    orig_leds = read(0x14, 504)
    new = bytearray(orig_leds)
    new[W * 4 + 1:W * 4 + 4] = b"\x01\x02\x03"
    try:
        r = write(0x24, bytes(new))
        ok = read(0x14, 504) == bytes(new)
        log("leds read back", "exact" if ok else "DIFFERS", f"(retries {r})")
    finally:
        write(0x24, orig_leds)
        restored = read(0x14, 504) == orig_leds
        log("leds restore", "EXACT" if restored else "DIFFERS")
    results["per_key_store"] = ok and restored
    log("PER-KEY COLOURS", "OK" if results["per_key_store"] else "FAILED")
    if not results["per_key_store"]:
        raise SystemExit

    # 4. DKS + key records, unchanged
    dks, keys = read(0x18, 1024), read(0x12, 512)
    write(0x28, dks)
    write(0x22, keys[:504])
    ok = read(0x18, 1024) == dks and read(0x12, 512) == keys
    results["advanced_keys"] = ok
    log("ADVANCED KEYS (no-op rewrite)", "OK" if ok else "FAILED")
except TimeoutError as e:
    log("TIMEOUT:", e)
finally:
    log("results:", results)
