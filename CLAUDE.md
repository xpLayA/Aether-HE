# Aether HE (private fork): Rust controller for the AULA F75 HE

Desktop + CLI control for the **AULA F75 HE** Hall-effect keyboard (USB `38A6:2729`) on Linux
(CachyOS, Wayland/Hyprland). Rust workspace; the original Python app lives in `legacy/python/` as the
protocol reference. Private repo: `github.com/xpLayA/aether-he` (forked from MrWhosNexus/Aether-HE,
upstream remote removed, upstream auto-updater disabled).

## Board safety: read before touching anything that writes
- **Never send** flash (`0x4F`), factory reset (`0x0F`) or calibration (`0x64`/`0x65`). In `aether-proto`
  they are not representable as `Cmd` values, and `guard()` rejects them again at the wire. Keep it that way.
- Only commands **verified on the F75** exist in `Cmd`. Adding one needs a guarded hardware probe first
  (read-only if possible; on/off pairs always sent in a `finally`), with the user's OK.
- Every write: `validate_actuation` → `Backup::snapshot` → write → **read back and compare**. Partial
  table reads abort before any write (all-or-nothing paging).
- Close the Aula web driver (browser) before probing; it holds the device and makes writes look
  accepted while nothing happens. Judge success only by a `0x55` reply or a visible effect.
- Hardware tests: back up first, restore after, and tell the user what to watch for.

## Layout
```
crates/aether-proto    HFD frame builders/parsers, no I/O; golden-tested vs legacy/python/protocol_mini60.py
crates/aether-device   hidraw transport (usage page 0xFF68 only), Keyboard trait, Hfd driver,
                       registry + layouts (data/), Backup, Profile, PRESETS
crates/aether-effects  host-streamed per-key effects (wave, breathe, aurora, comet, tide, twinkle, rain, striation, static;
                       reactive: ripple, splash, cross, sparks, afterglow, heatmap, lightning — fed by Engine::press)
crates/aether-cli      `aether` binary
crates/aether-gui      `aether-gui` (Slint 1.18, fluent-dark): ui/*.slint = layout only (UiState/Actions globals);
                       src/main.rs = glue; src/worker.rs owns the device (UI polls it every 33 ms)
data/                  boards.json (Rust registry), layouts/aula-f75he.json (80 keys, vendor table `t`),
                       shortcuts/<slug>.json (Fn shortcuts from the paper manual, "Fn shortcuts" page)
fixtures/f75/          real F75 replies + probe notes (used by tests)
packaging/             70-aether.rules (udev uaccess; must sort before 73-seat-late), .desktop, icon
scripts/               build.sh, install.sh, uninstall.sh
legacy/python/         original app + 640 pytest tests (reference only); docs/ = protocol research
```

## Build / run / test
```
./scripts/build.sh            # deps check, cargo test, release build
./target/release/aether-gui   # AETHER_PAGE=actuation opens a page (screenshots)
./target/release/aether info | light get | actuation get | profile list | backups
./scripts/install.sh          # ~/.local + app menu + udev rule (sudo); manifest-based
./scripts/uninstall.sh        # asks: keep keyboard settings or restore original; --purge deletes backups
cargo test --workspace        # golden frames, fake-board driver tests, profiles, effects
```
Regenerate golden frames after changing the Python reference:
`venv-web/bin/python legacy/python/tools/dump_golden_mini60.py crates/aether-proto/tests/golden/mini60.json`

## F75 HE protocol (HFD family, same as MINI 60 HE PRO)
- Interface 2, usage page `0xFF68` usage `0x61`, no report ID, 64-byte frames.
  Header `[0]=AA [1]=cmd [2]=len [3..5]=LE16 table offset [6]=last-chunk`, payload from byte 8
  (56-byte pages). Replies echo the header with `0x55`.
- `0x10` device info (VID/PID LE at 12/14, firmware at 16 → V1.55).
- `0x13`/`0x23` lighting: `[8]` mode, `[9..12]` RGB+alpha, `[16]` colorMode (1 = firmware colours),
  `[17]` brightness 0–5, `[18]` speed 0–5, `[22..24]` `AA 55`. The read-back keeps the stored colour,
  so restore from the saved block rather than re-reading. Modes: 0 off, 1 static … 20 custom (registry).
- `0x17`/`0x27` actuation: 126 records × 8 B `[axis][flags bit0=RT][trip][press][release]`, LE16 in
  0.01 mm, range 0.10–3.40 mm (RT 0.01–3.40). **All-zero record = firmware default** (factory state).
  Compare limits in raw 0.01 mm units (slider float error).
- `0x32` live per-key stream (9 pages of `[idx,r,g,b]`, ~28 fps, needs mode 20), `0x36` ends it.
- `0x14`/`0x24` stored per-key colours (same 504-byte `[idx,r,g,b]` table, shown in mode 20). `0x24`
  verified on the F75 2026-10-21: every page acked `0x55`, `0x14` read-back byte-exact, restore exact
  (`docs/context/f75-bringup/f75_custom_led_rt.py`). Capability `per_key_store`; in backups and profiles.
- Key presses for reactive effects: the board's keyboard collections (if0 boot `[mods][0][6 usages]`,
  if1 report 1 = NKRO bitmap of usages 0x00..0x77), read-only, `aether-device/src/keys_in.rs`.
- `0x12`/`0x22` key records `[type][p1][p2][p3]` (2 key, 8 DKS slot, 9 MT tap/hold/delay÷10ms, 0A TGL,
  0B SOCD mode/a/b and 0C RS 0/a/b on BOTH keys of a pair; usages, not indices). `0x18`/`0x28` DKS table,
  64 × 16 B, points in 0.1 mm. Write DKS before keys. All wire-verified on the F75 2026-10-21
  (`f75_keytable_rt.py`); F1–F12 base records are empty and "off" restores the pre-Aether record
  (`Backup::original_key_table`). SOCD modes: 3 last input, 1 key 1, 2 key 2, 4 neutral.
- **2.4 GHz receiver** `0C45:FEFE` "AULA F75 HE Dongle" (PID shared across AULA boards, matched by product
  string; `links` in boards.json): same HFD protocol on if3, usage page `0xFF60`, **32-byte** reports, so
  chunks are 24 bytes (`repage()`); each write page waits for its `0x55` ack. `0x10` reports the wired
  ID 38A6:2729; payload[16] work mode (02 = wireless), [17] battery %, [18] charge status (1/2 =
  charging). The board sleeps after a short idle time, and requests don't wake it (only a keypress does), so a
  timeout over RF means asleep. Verified 2026-10-22 (`f75_rf_probe.py`, `f75_rf_write_rt.py`): every
  table reads, and 0x23/0x27/0x24/0x28/0x22 write and restore byte-exact (`wireless_capabilities`). `0x32`
  streaming is dropped over RF (MINI 60), so host effects are wired-only; `0x35` RGB565 stream untried.
- **Bluetooth LE** (channel 1, "AULA F75 HE-1", HID `0005:0C45:FEFE`, the receiver's IDs): the report map
  has only keyboard/mouse/consumer/digitizer collections, no vendor collection and no feature reports
  (`fixtures/f75/bt_report_descriptor.txt`, 2026-10-22). No control channel over BT, so typing only.
  The battery comes from the GATT Battery Service (BlueZ → UPower), not 0x10. Aether recognises it anyway
  (`aether-device/src/bluetooth.rs`: hidapi bus Bluetooth + `bluetooth.name_prefix`, never opened; battery
  from `org.bluez.Battery1` over zbus). `connect()` returns `Error::BluetoothOnly`, the GUI shows
  connection state 3 and keeps retrying every 5 s, so it picks up the cable or receiver as soon as one appears.
- **No live key depth**: `0x66` travel-test is acked but streams nothing; the vendor's depth view uses
  calibration mode, which we never send. `0x60` untried.

## Verified status (F75 HE, 2026-10-21)
Lighting ✔ · host effects ✔ (incl. reactive) · stored per-key colours ✔ (wire-verified) · actuation /
rapid trigger ✔ (stored and felt) · advanced keys DKS/MT/TGL/SOCD/RS ✔ (wire-verified; feel pending) ·
backups, profiles, reset-to-default ✔. Not implemented / unverified: plain remap UI, macros, dead zones,
calibration, live depth, host effects over 2.4 GHz (lighting/actuation/keys/battery over the dongle ✔). DKS "hold" bits are source-only (never captured).

## Data on disk
Backups `~/.local/share/aether-he/backups/<board>-<unix>.json`: only the original (pre-Aether) state is
kept automatically. `Backup::snapshot` creates it before the first change and fills in sections it lacks,
but never overwrites it. Manual backups (`aether backup`, GUI "Backup now") add extra files.
`aether backups --prune` merges everything into the original and deletes the rest. Custom profiles `~/.config/aether-he/profiles/*.json`
(format 2: optional lighting + actuation table + optional effect); built-ins (Firmware default, Competitive,
Gaming, Balanced, Typing) are generated from `PRESETS`, set actuation only and are never saved. The GUI
always has an active profile: every edit is auto-saved into the active custom profile, and editing while a
built-in is active forks "<name> (custom)". GUI state `~/.config/aether-he/gui.json`.
Background: closing the window hides it while a host effect runs (`on_close_requested`), Quit/SIGTERM
quit cleanly (lighting restored, then the Settings "When Aether quits" mode, `quit_mode` in gui.json, is
written if set; on the next connect the active profile's lighting is put back). One instance per session via `$XDG_RUNTIME_DIR/aether-he.sock`
(`instance.rs`; a second launch just shows the window). `--background` maps the window for 0.5 s then
hides it: winit/Wayland can't later show a window that was never mapped. Autostart file
`~/.config/autostart/aether-he.desktop`. Auto-reconnect every 5 s while disconnected.

## Conventions
Match the surrounding style; keep comments for the why. Commit per logical step and push to `origin`
(the private repo) when the user asks. Security audit: `docs/SECURITY_AUDIT.md`.
