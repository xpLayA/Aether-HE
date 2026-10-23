"""Travel-test probe: 0x66 on -> collect 0xFB reports -> ALWAYS 0x67 off.
Only 0x66 and 0x67 may be sent; everything else raises before the wire."""
import time, json
from f75_common import *

dev = Dev({0x66, 0x67})
dev.open()

def frame(cmd):
    f = [0] * 64
    f[0], f[1] = 0xAA, cmd       # vendor SDK L(cmd, 0, 0): len 0, offset 0, flag 0
    return [0] + f               # report id 0

# drop anything stale
for _ in range(64):
    if not dev.read(64, 2):
        break

reports, ack = [], None

# Passive listeners on the board's OTHER interfaces: count only 0x55 0xFB frames.
import threading, hid as _hid
other_fb = {}
stop = threading.Event()
def listen(path, label):
    d = _hid.device(); d.open_path(path)
    n = 0
    while not stop.is_set():
        r = d.read(64, 50)
        if r and ((r[0] == 0x55 and len(r) > 1 and r[1] == 0xFB) or (len(r) > 2 and r[1] == 0x55 and r[2] == 0xFB)):
            n += 1
    other_fb[label] = n
    d.close()
threads = []
for info in _hid.enumerate(VID, PID):
    if info["usage_page"] != UP:
        lbl = "iface%d up=0x%04X usage=0x%02X" % (info["interface_number"], info["usage_page"], info["usage"])
        t = threading.Thread(target=listen, args=(info["path"], lbl), daemon=True); t.start(); threads.append(t)
time.sleep(2)
print("SENDING 0x66 (travel test on)", flush=True)
t0 = time.monotonic()
try:
    dev.write(frame(0x66))
    while time.monotonic() - t0 < 16.0:
        r = dev.read(64, 50)
        if not r:
            continue
        r = list(r)
        if r[0] == 0x00 and len(r) > 1 and r[1] == 0x55:
            r = r[1:]
        dt = round(time.monotonic() - t0, 3)
        if r[0] == 0x55 and r[1] == 0x66 and ack is None:
            ack = (dt, r[:16])
        reports.append((dt, r))
finally:
    dev.write(frame(0x67))
    time.sleep(0.3)
    tail = []
    for _ in range(64):
        r = dev.read(64, 20)
        if not r:
            break
        tail.append(list(r))
    dev.close()
    stop.set()
    for t in threads: t.join(1)
    print("SENT 0x67 (travel test off)", flush=True)
    print("0xFB frames on other interfaces:", other_fb)

fb = [(dt, r) for dt, r in reports if r[0] == 0x55 and r[1] == 0xFB]
other = [(dt, r[:12]) for dt, r in reports if not (r[0] == 0x55 and r[1] == 0xFB)]
print("ack:", ack)
print("total reports:", len(reports), " 0xFB:", len(fb), " other:", len(other))
for dt, r in other[:6]:
    print("  other", dt, bytes(r).hex(" "))
le = lambda r, i: r[i] | r[i + 1] << 8
keys = {}
for dt, r in fb:
    k = r[2]
    keys.setdefault(k, []).append((dt, r[3], le(r, 4), le(r, 6) & 0x7FFF, le(r, 8), le(r, 10), le(r, 12)))
for k, v in sorted(keys.items()):
    strokes = [x[5] for x in v]
    print(f"key {k:3}: {len(v):4} reports  stroke min {min(strokes)} max {max(strokes)} (0.01 mm)  "
          f"maxStroke {v[0][6]}  adc {v[0][4]}..{max(x[4] for x in v)}  status {v[0][1]}")
for dt, r in fb[:3]:
    print("  sample", dt, bytes(r[:16]).hex(" "))
print("after 0x67, reports still arriving:", sum(1 for r in tail if r[1] == 0xFB or (r[0] == 0 and r[2] == 0xFB)))
json.dump({"ack": ack, "reports": reports, "tail": tail}, open(OUT + "/travel_probe.json", "w"))
