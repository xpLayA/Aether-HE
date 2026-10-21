"""Step 1: READ-ONLY probe. Only GET commands (0x10-0x17) can be sent."""
from f75_common import *

drv, dev = driver(READ_CMDS)
info = drv.device_info(); print("device_info:", info)
light = drv.get_lighting(); print("lighting:", light)
act = drv._read_table(list(pm.iter_actuation_reads(64)), pm.ACT_TABLE_SIZE)
keyt = drv._read_key_table()
led = drv._read_table(list(pm.build_custom_led_read_frames(64)), pm.CUSTOM_LED_TABLE_SIZE)
cfg = drv._read_table(pm.build_config_read_frames(64), pm.CONFIG_TABLE_SIZE)
lrep = drv._transact(pm.build_light_read(64))
recs = drv._read_actuation_records()
pop = [i for i, r in enumerate(recs) if r is not None]
print("actuation populated slots:", len(pop), pop)
print("sample records:", {i: recs[i] for i in pop[:3]})
save("backup.json", {"device_info": info, "lighting": light, "light_raw": list(lrep),
                     "act_table": act.hex(), "key_table": keyt.hex(), "led_table": led.hex(),
                     "config_table": cfg.hex(), "populated": pop})
print("cmds sent:", sorted(set(dev.sent)))
dev.close()
