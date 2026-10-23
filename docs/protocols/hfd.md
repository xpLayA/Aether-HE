# HFD protocol family

The settings protocol of the HFD controller used by several AULA Hall-effect boards: the AULA F75 HE
(verified in Rust), and the MINI 60 HE PRO (verified in the legacy Python app). Implemented in
`crates/aether-proto` (frames, no I/O) and `crates/aether-device/src/hfd.rs` (driver, `"protocol": "hfd"`).

Every command listed here was round-tripped on an F75 unless marked otherwise. Per-board facts (IDs,
quirks, dates) live in `docs/boards/<slug>.md`.

## Transport
- Vendor collection: usage page `0xFF68` usage `0x61` on the cable (F75: interface 2); `0xFF60` on the
  2.4 GHz receiver (interface 3). No report ID.
- Frames: 64 bytes on the cable, **32 bytes** on the receiver.
  - Header: `[0]=AA [1]=cmd [2]=len [3..5]=LE16 table offset [6]=last-chunk`, payload from byte 8.
  - Pages: 56 bytes on the cable, 24 on the receiver (`repage()`).
  - Replies echo the header with `0x55` in byte 0.
  - Over the receiver, each write page waits for its `0x55` ack.
- Reads are `0x1X`, the matching writes `0x2X`. A table is read page by page; a partial read aborts
  before any write (all-or-nothing paging).

## Commands

| Read / write | What | Layout |
|---|---|---|
| `0x10` | device info | VID/PID LE at payload 12/14 (the wired ID even over RF), firmware at 16. Over RF: [16] work mode (02 = wireless), [17] battery %, [18] charging (1/2) |
| `0x13` / `0x23` | lighting | `[8]` mode, `[9..12]` RGB+alpha, `[16]` colorMode (1 = firmware colours), `[17]` brightness, `[18]` speed, `[22..24]` `AA 55`. The read-back keeps the stored colour, so restore from the saved block, not a re-read |
| `0x17` / `0x27` | actuation | 126 records × 8 B `[axis][flags bit0=RT][trip][press][release]`, LE16 in 0.01 mm. **All-zero record = firmware default.** Compare limits in raw 0.01 mm units |
| `0x14` / `0x24` | stored per-key colours | 504-byte `[idx,r,g,b]` table (126 slots), shown in lighting mode 20 (custom). Kept in flash: never use it to animate |
| `0x12` / `0x22` | key records | `[type][p1][p2][p3]`: 2 key, 8 DKS slot, 9 mod-tap tap/hold/delay÷10 ms, 0A toggle, 0B SOCD mode/a/b and 0C rapid snap 0/a/b on **both** keys of a pair. Usages, not indices. SOCD modes: 3 last input, 1 key 1, 2 key 2, 4 neutral |
| `0x18` / `0x28` | DKS table | 64 × 16 B, points in 0.1 mm. Write DKS before key records. "Hold" bits are source-only (never captured) |
| `0x32` | live per-key stream | 9 pages of `[idx,r,g,b]`, ~28 fps, needs lighting mode 20; nothing stored. `0x36` ends it. **Dropped by the RF bridge** (acked, never shown) |
| `0x35` | RGB565 music stream | untried |
| `0x66` | travel test | acked, streams nothing; the vendor's depth view uses calibration mode |

## Never sent
Flash `0x4F`, factory reset `0x0F`, calibration `0x64`/`0x65`. They can't be expressed as a `Cmd`,
and `guard()` refuses them again at the wire.

## Key presses (reactive effects)
These are read from the board's ordinary keyboard collections, not the vendor channel, and only ever read
(`crates/aether-device/src/keys_in.rs`): if0 boot report `[mods][0][6 usages]`, if1 report 1 = NKRO
bitmap of usages `0x00..0x77`.

## Golden tests
`crates/aether-proto/tests/golden/mini60.json` holds frames from the Python reference
(`legacy/python/protocol_mini60.py`). To regenerate after changing that reference:
`venv-web/bin/python legacy/python/tools/dump_golden_mini60.py crates/aether-proto/tests/golden/mini60.json`.

## Sources
`docs/context/issue-6-mini60-pro-raw/HFD_SDK_DECODE.md` (command table from the vendor SDK),
`WIRELESS_*.md`, the probe scripts in `docs/context/f75-bringup/`.
