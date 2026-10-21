"""Distinct stream test: 4 s solid blue, then a single green key walking the layout order."""
import time
from f75_common import *
lay = json.load(open(ROOT + "/ui/layouts/aula-f75he.json"))["keys"]
order = [int(k["index"]) for k in sorted(lay, key=lambda k: (float(k["y"]), float(k["x"])))]
drv, dev = driver(READ_CMDS | {0x23, 0x32, 0x36}, lighting={"hostEngine": True})

def light_block():
    drv._drain()
    return list(drv._transact(pm.build_light_read(64)))

time.sleep(5)
orig = light_block()
try:
    drv.begin_host_stream()
    t0 = time.monotonic()
    while time.monotonic() - t0 < 4:
        drv.stream_frame({i: (0, 0, 255) for i in order}); time.sleep(0.03)
    for i in order:
        t1 = time.monotonic()
        while time.monotonic() - t1 < 0.2:
            drv.stream_frame({i: (0, 255, 0)}); time.sleep(0.03)
    print("walked", len(order), "keys")
finally:
    drv.end_host_stream(); time.sleep(0.2)
    f = pm.build_light(orig[8], orig[9], orig[10], orig[11], alpha=orig[12],
                       color_mode=orig[16], brightness=orig[17], speed=orig[18])
    f[13:16] = orig[13:16]
    drv._send(f); time.sleep(0.3)
    print("RESTORE EXACT" if light_block()[8:22] == orig[8:22] else "RESTORE DIFFERS")
    dev.close()
