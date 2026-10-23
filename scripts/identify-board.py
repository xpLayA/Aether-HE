#!/usr/bin/env python3
"""List every HID interface with what a port to Aether needs: VID:PID, USB interface, the usage
pages in its report descriptor (vendor pages 0xFF00+ are where settings protocols live), report
IDs and the largest report size. Read-only: it only reads /sys, it never opens a device.

usage: scripts/identify-board.py [name filter] [--dump]   (--dump also prints the raw descriptor)
"""
import glob, os, sys

args = [a for a in sys.argv[1:] if not a.startswith("--")]
dump = "--dump" in sys.argv
want = args[0].lower() if args else ""


def parse(desc):
    """Usage pages, report IDs and the largest report (bytes) per direction, from a HID descriptor."""
    pages, ids, size, count, sizes, rid = [], set(), 0, 0, {}, 0
    i = 0
    while i < len(desc):
        b = desc[i]
        if b == 0xFE:  # long item
            i += 3 + desc[i + 1]
            continue
        n = (0, 1, 2, 4)[b & 3]
        v = int.from_bytes(desc[i + 1:i + 1 + n], "little")
        tag = b & 0xFC
        if tag == 0x04:
            pages.append(v)
        elif tag == 0x84:
            rid = v
            ids.add(v)
        elif tag == 0x74:
            size = v
        elif tag == 0x94:
            count = v
        elif tag in (0x80, 0x90, 0xB0):  # Input, Output, Feature
            kind = {0x80: "in", 0x90: "out", 0xB0: "feature"}[tag]
            key = (kind, rid)
            sizes[key] = sizes.get(key, 0) + size * count
        i += 1 + n
    big = {}
    for (kind, _), bits in sizes.items():
        big[kind] = max(big.get(kind, 0), (bits + 7) // 8)
    return pages, sorted(ids), big


for node in sorted(glob.glob("/sys/class/hidraw/hidraw*"), key=lambda p: int(p.rsplit("hidraw", 1)[1])):
    dev = os.path.realpath(os.path.join(node, "device"))
    try:
        uevent = dict(l.split("=", 1) for l in open(os.path.join(dev, "uevent")).read().splitlines() if "=" in l)
    except OSError:
        continue
    name = uevent.get("HID_NAME", "?")
    if want and want not in name.lower():
        continue
    bus, vid, pid = uevent.get("HID_ID", "0:0:0").split(":")
    bus = {"0003": "usb", "0005": "bluetooth"}.get(bus[-4:], bus)
    iface = dev.split("/")[-2].rsplit(".", 1)[-1] if ":" in dev.split("/")[-2] else "?"
    desc = open(os.path.join(dev, "report_descriptor"), "rb").read()
    pages, ids, big = parse(desc)
    vendor = [p for p in pages if p >= 0xFF00]
    print(f"/dev/{os.path.basename(node)}  {name}")
    print(f"  {bus}  {int(vid, 16):04X}:{int(pid, 16):04X}  interface {iface}")
    print(f"  usage pages: {', '.join(f'0x{p:04X}' for p in dict.fromkeys(pages)) or '-'}"
          + (f"   <- vendor: {', '.join(f'0x{p:04X}' for p in dict.fromkeys(vendor))}" if vendor else ""))
    print(f"  report IDs: {ids or 'none'}   largest report (bytes): "
          + ", ".join(f"{k} {v}" for k, v in big.items()))
    if dump:
        print("  descriptor:", desc.hex(" "))
    print()
