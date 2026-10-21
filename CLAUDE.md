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
crates/aether-effects  host-streamed per-key effects (wave, breathe, aurora, comet, tide, twinkle, rain, striation, static)
crates/aether-cli      `aether` binary
crates/aether-gui      `aether-gui` (Slint 1.18, fluent-dark): ui/*.slint = layout only (UiState/Actions globals);
                       src/main.rs = glue; src/worker.rs owns the device (UI polls it every 33 ms)
data/                  boards.json (Rust registry), layouts/aula-f75he.json (80 keys, vendor table `t`)
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
- `0x12` key table read-only so far (matches the layout; F1–F12 base records empty = default).
- **No live key depth**: `0x66` travel-test is acked but streams nothing; the vendor's depth view uses
  calibration mode, which we never send. `0x60` untried.

## Verified status (F75 HE, 2026-10-21)
Lighting ✔ · host effects ✔ · actuation / rapid trigger ✔ (stored and felt) · backups, profiles,
reset-to-default ✔. Not implemented / unverified: remap, macros, SOCD, advanced keys, dead zones,
calibration, live depth, 2.4 GHz dongle.

## Data on disk
Backups `~/.local/share/aether-he/backups/<board>-<unix>.json` (taken before every change; the oldest
is the pre-Aether state, used by "Restore original"). Profiles `~/.config/aether-he/profiles/*.json`
(lighting + actuation table + optional effect). GUI state `~/.local/share/aether-he/app.ron`.

## Conventions
Match the surrounding style; keep comments for the why. Commit per logical step and push to `origin`
(the private repo) when the user asks. Security audit: `docs/SECURITY_AUDIT.md`.
