"""0x24 per-key LED table round-trip on the F75 (MINI 60-verified, never sent to the F75 before).

read 0x13 lighting + 0x14 table -> write a test pattern with 0x24 (each page must be acked 0x55)
-> mode 20 -> read 0x14 back and compare -> hold HOLD_S seconds (watch: rainbow across the keys)
-> finally: write the original table back, restore the exact lighting block, verify both.
"""
import sys, time
from f75_common import *

HOLD_S = float(sys.argv[1]) if len(sys.argv) > 1 else 8.0
drv, dev = driver(READ_CMDS | {0x23, 0x24})


def light_block():
    drv._drain()
    return list(drv._transact(pm.build_light_read(64)))


def led_table():
    return drv._read_table(list(pm.build_custom_led_read_frames(64)), pm.CUSTOM_LED_TABLE_SIZE)


def write_leds(table):
    drv._drain()
    for f in pm.build_custom_led_write_frames(list(table), 64):
        rep = drv._transact(f)  # raises if a page isn't acked
        assert rep[0] == 0x55 and rep[1] == 0x24, rep[:8]


orig_light = light_block()
orig_led = led_table()
save("custom_led_before.json", {"light": orig_light, "led": orig_led.hex()})
print("lighting before:", bytes(orig_light[8:24]).hex(" "))
print("0x14 before    : nonzero bytes", sum(1 for b in orig_led if b))

# hue sweep by key index, so a wrong record order shows as broken stripes
def hue(i):
    h = (i * 6.0 / 96) % 6
    x = int(255 * (1 - abs(h % 2 - 1)))
    return [(255, x, 0), (x, 255, 0), (0, 255, x), (0, x, 255), (x, 0, 255), (255, 0, x)][int(h)]

pattern = bytes(pm.build_custom_led_table([hue(i) for i in range(pm.CUSTOM_LED_COUNT)]))
try:
    write_leds(pattern)
    print("0x24 write     : every page acked 0x55")
    drv._send(pm.build_light(20, 255, 255, 255, alpha=0xFF, color_mode=0,
                             brightness=orig_light[17], speed=orig_light[18]))
    time.sleep(0.3)
    back = led_table()
    print("0x14 read-back :", "MATCHES" if back == pattern else
          f"DIFFERS in {sum(1 for a, b in zip(back, pattern) if a != b)} bytes")
    print(f"holding {HOLD_S}s: the keys should show a rainbow from Esc rightwards")
    time.sleep(HOLD_S)
finally:
    write_leds(orig_led)
    f = pm.build_light(orig_light[8], orig_light[9], orig_light[10], orig_light[11], alpha=orig_light[12],
                       color_mode=orig_light[16], brightness=orig_light[17], speed=orig_light[18])
    f[13:16] = orig_light[13:16]
    drv._send(f)
    time.sleep(0.3)
    print("restore table  :", "EXACT" if led_table() == orig_led else "DIFFERS")
    after = light_block()
    print("restore light  :", "EXACT" if after[8:22] == orig_light[8:22] else "DIFFERS", bytes(after[8:24]).hex(" "))
    print("cmds sent:", sorted({hex(c) for c in dev.sent}))
    dev.close()
