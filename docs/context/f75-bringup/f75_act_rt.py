"""Actuation round-trip on W (index 34): fixed 1.50 -> RT 0.30 -> restore full table byte-identical."""
import time
from f75_common import *
W = 34
drv, dev = driver(READ_CMDS | {0x27})

def raw_table():
    return drv._read_table(list(pm.iter_actuation_reads(64)), pm.ACT_TABLE_SIZE)

orig_raw = raw_table()
orig_recs = drv._read_actuation_records()
save("act_before.json", orig_raw.hex())
print("orig nonzero bytes:", sum(1 for b in orig_raw if b))
try:
    drv.set_actuation([W], 0, 1.50); time.sleep(0.2)
    r = drv._read_actuation_records()
    print("fixed :", r[W], "| other slots changed:", [i for i in range(len(r)) if i != W and r[i] != orig_recs[i]])
    drv.set_actuation([W], 12, 1.50, 0.30, 0.30); time.sleep(0.2)
    r = drv._read_actuation_records()
    print("rt    :", r[W], "| other slots changed:", [i for i in range(len(r)) if i != W and r[i] != orig_recs[i]])
finally:
    with drv.transaction():
        drv._write_actuation_records(orig_recs)
    time.sleep(0.2)
    after = raw_table()
    print("RESTORE BYTE-IDENTICAL" if after == orig_raw else f"RESTORE DIFFERS ({sum(1 for a,b in zip(after,orig_raw) if a!=b)} bytes)")
    print("cmds sent:", sorted({hex(c) for c in dev.sent}))
    dev.close()
