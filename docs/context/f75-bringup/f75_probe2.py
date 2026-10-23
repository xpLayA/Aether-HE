"""READ-ONLY: raw 0x17 replies + key-table vs layout cross-check."""
from f75_common import *
drv, dev = driver(READ_CMDS)
drv._drain()
for f in list(pm.iter_actuation_reads(64))[:3]:
    print("REQ ", bytes(f[:8]).hex(" "))
    r = drv._transact(f)
    print("REPL", bytes(r[:24]).hex(" "), "... nonzero", sum(1 for x in r[8:] if x))
keyt = drv._read_key_table()
pop = sorted(i for i in range(len(keyt)//4) if any(keyt[i*4:i*4+4]))
lay = sorted(int(k["index"]) for k in json.load(open(ROOT + "/ui/layouts/aula-f75he.json"))["keys"])
print("key-table populated:", len(pop))
print("in layout not table:", sorted(set(lay) - set(pop)))
print("in table not layout:", {i: keyt[i*4:i*4+4].hex(" ") for i in sorted(set(pop) - set(lay))})
for i in (0, 34, 85, 106):
    print(i, keyt[i*4:i*4+4].hex(" "))
print("cmds sent:", sorted({hex(c) for c in dev.sent}))
dev.close()
