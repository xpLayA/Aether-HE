"""Host stream test: custom mode -> 10 s rainbow via 0x32 -> 0x36 clear -> exact lighting restore."""
import colorsys, time
from f75_common import *
lay = json.load(open(ROOT + "/ui/layouts/aula-f75he.json"))["keys"]
xs = {int(k["index"]): float(k["x"]) for k in lay}
xmax = max(xs.values())
drv, dev = driver(READ_CMDS | {0x23, 0x32, 0x36}, lighting={"hostEngine": True})

def light_block():
    drv._drain()
    return list(drv._transact(pm.build_light_read(64)))

time.sleep(5)
orig = light_block()
frames = 0; errors = 0; times = []
try:
    drv.begin_host_stream()
    t0 = time.monotonic()
    while time.monotonic() - t0 < 10:
        ts = time.monotonic()
        ph = (ts - t0) * 0.5
        cols = {i: tuple(int(c * 255) for c in colorsys.hsv_to_rgb((x / xmax - ph) % 1, 1, 1))
                for i, x in xs.items()}
        try:
            drv.stream_frame(cols); frames += 1
        except Exception as e:
            errors += 1; print("err", e)
        times.append(time.monotonic() - ts)
    el = time.monotonic() - t0
    times.sort()
    print(f"frames {frames} in {el:.2f}s = {frames/el:.1f} fps, errors {errors}, "
          f"p50 {times[len(times)//2]*1000:.1f} ms, p95 {times[int(len(times)*.95)]*1000:.1f} ms")
finally:
    drv.end_host_stream()
    time.sleep(0.2)
    f = pm.build_light(orig[8], orig[9], orig[10], orig[11], alpha=orig[12],
                       color_mode=orig[16], brightness=orig[17], speed=orig[18])
    f[13:16] = orig[13:16]
    drv._send(f); time.sleep(0.3)
    after = light_block()
    print("RESTORE EXACT" if after[8:22] == orig[8:22] else f"RESTORE DIFFERS {bytes(after[8:22]).hex(' ')}")
    print("cmds sent:", sorted({hex(c) for c in dev.sent}))
    dev.close()
