"""0x22 key table + 0x28 DKS table round-trips on the F75 (MINI 60-verified, never sent to the F75 before).

Saves the original 0x12 and 0x18 tables, then, each step verified by every page being acked 0x55
and a byte-exact read-back:
  1. 0x22 writes the unchanged key table back (no-op)
  2. TGL on F12 (index 12)          0A 00 00 00
  3. MT on F12: tap F12, hold F11, 250 ms
  4. RS pair F11 (11) + F12 (12)
  5. SOCD pair F11 + F12, mode 3 (last input priority)
  6. 0x18 read, 0x28 writes the unchanged DKS table back (no-op)
  7. DKS slot 0 bound to F12 (0x28 first, then 0x22, as the vendor does)
finally: write both original tables back and verify they read back byte-exact.
F11/F12 are used because they are the least likely keys to be pressed meanwhile.
"""
import time
from f75_common import *

drv, dev = driver(READ_CMDS | {0x18, 0x22, 0x28})
KEY_WRITE = pm.KEY_TABLE_WRITE_SIZE
F11, F12 = 11, 12
U_F11, U_F12 = 0x44, 0x45


def key_table():
    return drv._read_table(list(pm.build_key_table_read_frames(64)), pm.KEY_TABLE_SIZE)


def dks_table():
    return drv._read_table(list(pm.build_dks_travel_read_frames(64)), pm.DKS_TABLE_SIZE)


def write_acked(frames, cmd):
    drv._drain()
    for f in frames:
        rep = drv._transact(f)
        assert rep[0] == 0x55 and rep[1] == cmd, bytes(rep[:8]).hex()


def put_keys(table):
    write_acked(pm.build_key_table_write_frames(list(table), 64), 0x22)
    time.sleep(0.1)
    back = key_table()
    return "MATCHES" if back[:KEY_WRITE] == bytes(table)[:KEY_WRITE] else \
        f"DIFFERS in {sum(1 for a, b in zip(back[:KEY_WRITE], bytes(table)[:KEY_WRITE]) if a != b)} bytes"


def put_dks(table):
    write_acked(pm.build_dks_travel_write_frames(list(table), 64), 0x28)
    time.sleep(0.1)
    back = dks_table()
    return "MATCHES" if back == bytes(table) else \
        f"DIFFERS in {sum(1 for a, b in zip(back, bytes(table)) if a != b)} bytes"


orig_keys = key_table()
orig_dks = None
save("keytable_before.json", {"key_table": orig_keys.hex()})
print("0x12 before: F11", orig_keys[F11 * 4:F11 * 4 + 4].hex(), "F12", orig_keys[F12 * 4:F12 * 4 + 4].hex(),
      "| tail 504..511:", orig_keys[KEY_WRITE:].hex())
try:
    print("1 no-op 0x22      :", put_keys(orig_keys))
    print("2 TGL F12         :", put_keys(pm.apply_key_record(orig_keys, F12, pm.key_record_tgl())))
    print("3 MT F12 250 ms   :", put_keys(pm.apply_key_record(orig_keys, F12, pm.key_record_mt(25, U_F12, U_F11))))
    print("4 RS F11+F12      :", put_keys(pm.apply_paired_key_record(orig_keys, F11, F12, pm.key_record_rs(U_F11, U_F12))))
    print("5 SOCD F11+F12 m3 :", put_keys(pm.apply_paired_key_record(orig_keys, F11, F12, pm.key_record_socd(U_F11, U_F12, 3))))
    put_keys(orig_keys)

    orig_dks = dks_table()
    save("dks_before.json", {"dks_table": orig_dks.hex()})
    print("0x18 before: nonzero slots", [i for i in range(64) if any(orig_dks[i * 16:i * 16 + 16])])
    print("6 no-op 0x28      :", put_dks(orig_dks))
    slot = pm.dks_slot_record([1.6, 3.0, 3.0, 1.6], [U_F12], [1, 1, 1, 1])
    dks = bytearray(orig_dks)
    dks[0:16] = bytes(slot)
    print("7 DKS slot 0      :", put_dks(dks), "/ key record:", put_keys(pm.apply_key_record(orig_keys, F12, pm.key_record_dks(0))))
finally:
    put_keys(orig_keys)
    print("restore 0x22      :", "EXACT" if key_table() == orig_keys else "DIFFERS")
    if orig_dks is not None:
        put_dks(orig_dks)
        print("restore 0x28      :", "EXACT" if dks_table() == orig_dks else "DIFFERS")
    print("cmds sent:", sorted({hex(c) for c in dev.sent}))
    dev.close()
