# AULA F75 HE hardware captures (2026-10-21)

Read from a physical F75 HE (38A6:2729, fw interface 0xFF68, 64-byte frames) with the
bring-up scripts in `docs/context/f75-bringup/`. Golden inputs for the Rust port's parsers.

- `backup.json` — full read-only snapshot: 0x10 device info, 0x13 lighting reply (`light_raw`,
  whole 64-byte frame), and the reassembled tables as hex: 0x17 actuation (1008 B, all zero =
  factory), 0x12 key table (512 B), 0x14 custom LED (504 B, all zero), 0x11 config (64 B).
- `light_before.json` — a 0x13 reply frame (stored mode 0x0B Drifting, colorMode 1, bri 5, speed 3).
- `act_before.json` — the 0x17 actuation table before the W round-trip (hex).

Verified outcomes: see the `aula-f75-he` entry's `_note`s in `data/board_registry.json`.

## Travel-test probe (2026-10-21): no live depth
`docs/context/f75-bringup/f75_travel_probe.py` sent the SDK's `SET_SIMULATION_TEST_ON` (`AA 66 00…`),
listened 9 s and 16 s, then always sent `SET_SIMULATION_TEST_OFF` (`AA 67 00…`). The board acked
(`55 66 00…`) within 3 ms but produced **no `0xFB` reports** on the 0xFF68 interface or passively on its
five other collections. The vendor UI never calls this mode; its live travel display uses calibration
mode (0x64, preceded by a calibration wipe), which Aether never sends. Live depth is therefore not
offered for this board. Not tried: `0x60 GET_MAGNETIC_AXIS_KEY_STATUS` (never referenced, format unknown).
