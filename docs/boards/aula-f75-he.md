# AULA F75 HE

75% Hall-effect board, HFD protocol ([`docs/protocols/hfd.md`](../protocols/hfd.md)). Data:
`data/boards/aula-f75-he/`. Tested unit: firmware V1.55, CachyOS with Hyprland.

## Connections

| Link | IDs | Settings channel | Status |
|---|---|---|---|
| USB-C | `38A6:2729` | interface 2, usage page `0xFF68`, 64-byte frames | everything below ✔ |
| 2.4 GHz receiver "AULA F75 HE Dongle" | `0C45:FEFE` (PID shared across AULA boards: matched by product string) | interface 3, usage page `0xFF60`, 32-byte frames | all tables ✔, host effects ✗ |
| Bluetooth LE "AULA F75 HE-<channel>" | `0005:0C45:FEFE` | **none**: the report map has only keyboard/mouse/consumer/digitizer collections, no vendor or feature reports (`fixtures/f75/bt_report_descriptor.txt`) | recognised, battery only |

## Verified (wired, 2026-10-21)

| Capability | How it was verified | Script |
|---|---|---|
| lighting | written, read back, restored; visible | `docs/context/f75-bringup/f75_light_rt.py` |
| host effects (incl. reactive) | visible, ~28 fps | `f75_stream.py`, `f75_stream2.py` |
| per-key colours (`0x24`) | every page acked, read-back byte-exact, restore exact | `f75_custom_led_rt.py` |
| actuation / rapid trigger | stored and felt | `f75_act_rt.py` |
| advanced keys DKS/MT/TGL/SOCD/RS | wire-verified (feel pending) | `f75_keytable_rt.py` |
| backups, profiles, reset to default | read-back | (app) |

- The F1–F12 base records are empty. Turning an advanced key "off" restores the pre-Aether record
  (`Backup::original_key_table`).
- Key presses for reactive effects come from if0 and if1 (see the protocol doc).

## 2.4 GHz receiver (2026-10-22/23)
- **Verified:** every table reads, and `0x23/0x27/0x24/0x28/0x22` write and restore byte-exact
  (`f75_rf_probe.py`, `f75_rf_write_rt.py`). That's `wireless_capabilities` in `board.json`.
- **Battery:** `0x10` payload [17], charging at [18].
- **Sleep:** the board sleeps after a short idle time. Requests don't wake it, only a keypress does, so a
  timeout over RF means it's asleep.
- **No host effects:** `0x32` is acked chunk by chunk, but the keys never change. On 2026-10-23
  (`f75_rf_stream_probe.py`, run twice) the keys showed only the stored custom colours. The GUI says
  "Effects need the USB cable".

## Bluetooth
- **No settings:** there's no control channel, so typing only.
- **Battery:** it comes from the GATT Battery Service (BlueZ `org.bluez.Battery1` over zbus), not
  `0x10`.
- **How Aether handles it:** it recognises the board by hidapi bus Bluetooth plus the
  `bluetooth.name_prefix` and never opens it (`crates/aether-device/src/bluetooth.rs`). `connect()`
  returns `Error::BluetoothOnly`. The GUI shows connection state 3 and retries every 5 s, so it
  takes over as soon as the cable or receiver appears.

## Not available
- **Live key depth:** `0x66` streams nothing, and the vendor's view uses calibration, which we never send.
  `0x60` is untried.
- **Not implemented yet:** plain remap UI, macros, dead zones, sleep timers.

## Files
- `fixtures/f75/`: real replies and the BT descriptor, used by tests.
- `data/boards/aula-f75-he/layout.json`: 80 keys, from the vendor driver's layout array `t`.
- `data/boards/aula-f75-he/shortcuts.json`: the paper manual's Fn page.
