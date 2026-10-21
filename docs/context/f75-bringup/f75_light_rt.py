"""Lighting round-trip: read -> static red -> read back -> restore exact bytes -> verify."""
import time
from f75_common import *
drv, dev = driver(READ_CMDS | {0x23})

def light_block():
    drv._drain()
    return list(drv._transact(pm.build_light_read(64)))

time.sleep(5)
orig = light_block()
save("light_before.json", orig)
print("before:", bytes(orig[8:24]).hex(" "))
try:
    drv._send(pm.build_light(1, 255, 0, 0, alpha=0xFF, color_mode=0,
                             brightness=orig[17], speed=orig[18]))
    time.sleep(0.3)
    red = light_block()
    print("red   :", bytes(red[8:24]).hex(" "))
    ok = red[8] == 1 and red[9:12] == [255, 0, 0]
    print("READBACK RED OK" if ok else "READBACK RED MISMATCH")
    time.sleep(15)
finally:
    f = pm.build_light(orig[8], orig[9], orig[10], orig[11], alpha=orig[12],
                       color_mode=orig[16], brightness=orig[17], speed=orig[18])
    f[13:16] = orig[13:16]
    drv._send(f)
    time.sleep(0.3)
    after = light_block()
    print("after :", bytes(after[8:24]).hex(" "))
    print("RESTORE EXACT" if after[8:22] == orig[8:22] else "RESTORE DIFFERS")
    print("cmds sent:", sorted({hex(c) for c in dev.sent}))
    dev.close()
