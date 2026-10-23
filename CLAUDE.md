# Aether HE

Native Linux app and CLI (Rust) for configuring Hall-effect keyboards over their vendor HID protocol:
lighting, host-streamed effects, actuation, rapid trigger, advanced keys, profiles and backups.
Supported board: **AULA F75 HE** (more can be added as data; see `docs/PORTING.md`). Developed on
CachyOS with Wayland/Hyprland. Repo `github.com/xpLayA/Aether-HE`, a GitHub fork of MrWhosNexus/Aether-HE
(Python, WIN60). That code is in `legacy/`, kept for reference only; nothing builds or tests from it.

**Before working on a board, read its notes:** `docs/boards/<slug>.md`, plus its protocol doc in
`docs/protocols/` (for example `hfd.md`).

## Board safety (read before anything that writes to a keyboard)
- **Never send** firmware flash, factory reset or calibration (HFD: `0x4F`, `0x0F`, `0x64`/`0x65`).
  They are not representable as `Cmd` values in `aether-proto`, and `guard()` refuses them again at the
  wire. Keep it that way, and do the same for any new protocol.
- `Cmd` holds only commands **verified on hardware**. A new one needs a guarded probe first (read-only
  if possible; on/off pairs always sent in a `finally`), with the user's OK.
- Every write goes `validate` → `Backup::snapshot` → write → **read back and compare**. Partial table
  reads abort before any write.
- Close the vendor's web driver before probing: it holds the device, and writes look accepted while
  nothing happens. Success means a byte-exact read-back or a visible effect, **never an ack alone**.
- For a hardware test: back up first, restore in a `finally`, and tell the user what to watch for.

## Architecture
```
aether-gui (Slint)    aether (CLI)
        \              /
       aether-device ── aether-effects        Keyboard trait, drivers, registry, backups, profiles
             |
       aether-proto                           frame builders/parsers, no I/O, golden-tested
             |
   hidraw (vendor usage page only)
```

| Path | Role |
|---|---|
| `crates/aether-proto` | HFD frames; `Cmd`, `guard()`, `repage()`; tables are 126 slots (`KEY_SLOTS`) |
| `crates/aether-device` | `transport.rs` (hidraw, vendor usage page only); `lib.rs` `Keyboard` trait + `connect()`/`open()` dispatching on `board.protocol`; `hfd.rs` driver (`require(capability)` gates every method); `data.rs` registry; `backup.rs`, `profile.rs` (`PRESETS`), `advanced.rs`, `keys_in.rs` (read-only key presses), `bluetooth.rs` (detect only) |
| `crates/aether-effects` | `Engine`: animated (wave, breathe, aurora, comet, tide, twinkle, rain, striation, static) and reactive (ripple, splash, cross, sparks, afterglow, heatmap, lightning; fed by `Engine::press`) |
| `crates/aether-cli` | the `aether` binary |
| `crates/aether-gui` | Slint 1.18, fluent-dark. `ui/*.slint` is layout only (`UiState`/`Actions` globals); `src/main.rs` is glue, state → UI in `push_*`; `src/worker.rs` owns the device (polled every 33 ms) |
| `data/boards/<slug>/` | `board.json` (IDs, links, protocol, **capabilities**, lighting modes, limits), `layout.json`, optional `shortcuts.json` |
| `docs/boards/`, `docs/protocols/` | per-board facts and protocol references |
| `docs/context/` | research, vendor captures, hardware probe scripts (`<board>-bringup/`) |
| `fixtures/<board>/` | real device replies used by tests |
| `scripts/` | `build.sh`, `install.sh`/`uninstall.sh` (manifest-based), `identify-board.py` (read-only HID survey) |
| `packaging/` | `70-aether.rules` (udev uaccess; must sort before 73-seat-late), `.desktop`, icon |

## Boards and capabilities
- A board is one folder, `data/boards/<slug>/`, and the folder name must equal the slug. Ports never
  edit a shared file.
- Capabilities (`lighting`, `host_effects`, `actuation`, `rapid_trigger`, `per_key_store`,
  `advanced_keys`) are `true` **only after a hardware round-trip**, recorded with its date and script
  in `_comment`. `wireless_capabilities` replaces them when the board is opened through a wireless
  link.
- Drivers refuse what isn't verified (`require`). The GUI shows only the pages a board's
  capabilities allow (`push_nav`) and never checks a board's name.
- Fn shortcuts come from the connected board's own `shortcuts.json`. Only `groups` is required; the
  F-row table, its system modes and its labels are optional.
- `every_shipped_board_is_consistent` (`data.rs`) checks every board folder. Adding a board:
  `docs/PORTING.md`. A new protocol: implement `Keyboard`, add it to `open()` and to that test.

## Build / run / test
```
./scripts/build.sh            # deps check, cargo test, release build
cargo test --workspace        # golden frames, fake-board driver tests, registry, profiles, effects
./target/release/aether-gui   # AETHER_PAGE=actuation opens a page; AETHER_SCROLL=600 scrolls it
./target/release/aether info | light get | actuation get | keys list | profile list | backups
./scripts/install.sh          # ~/.local + app menu + udev rule (sudo once)
./scripts/uninstall.sh        # asks: keep keyboard settings or restore original; --purge deletes backups
scripts/identify-board.py [name] [--dump]   # read-only: VID:PID, interface, vendor usage pages
```
`AETHER_DATA_DIR` overrides where `boards/` is found. Without it: installed share dir first, then the
source tree. Only one GUI runs per session, so quit the installed one before testing a build.

## Workflow
- **Never commit to `main`.** Branch first: `board/<slug>`, `feat/…`, `fix/…`, `docs/…`, `refactor/…`.
  Merge through a pull request (`gh pr create`) once CI passes. `main` is protected (PR + the "Test"
  check). Remotes: `origin` = xpLayA/Aether-HE; `upstream` = MrWhosNexus/Aether-HE, fetch only.
  Never push or open PRs there; `gh repo set-default` is xpLayA/Aether-HE.
- Conventional commits, one logical step each. The body says why, and names the probe script and
  date for anything verified on hardware.
- Push or open PRs only when the user asks. Never force-push without their explicit OK.
- Keep `README.md` (features, status table), `docs/boards/<slug>.md` and the protocol docs in step
  with the code.

## Runtime behaviour and data on disk
- **Backups** (`~/.local/share/aether-he/backups/<board>-<unix>.json`):
  - only the original, pre-Aether state is kept automatically: `Backup::snapshot` creates it before
    the first change, fills in sections it lacks, and never overwrites it;
  - manual backups (`aether backup`, "Backup now") add files;
  - `aether backups --prune` merges everything into the original.
- **Profiles** (`~/.config/aether-he/profiles/*.json`, format 2: optional lighting + actuation table +
  optional effect):
  - built-ins (Firmware default, Competitive, Gaming, Balanced, Typing) come from `PRESETS`, set
    actuation only, and are never saved;
  - the GUI always has an active profile and auto-saves every edit into it;
  - editing a built-in forks "<name> (custom)".
- **GUI state** is in `~/.config/aether-he/gui.json`. Autostart is
  `~/.config/autostart/aether-he.desktop` (Hyprland users add `exec-once = aether-gui --background`).
- **Closing the window** hides it while a host effect runs (`on_close_requested`).
- **Quit/SIGTERM** quits cleanly:
  1. the lighting is restored;
  2. the "When Aether quits" mode (`quit_mode`) is written if set;
  3. the next connect puts the profile's lighting back.
- **One instance** per session (`$XDG_RUNTIME_DIR/aether-he.sock`, `instance.rs`): a second launch
  shows the existing window.
- **`--background`** maps the window for 0.5 s, then hides it. winit on Wayland can't later show a
  window that was never mapped.
- **Auto-reconnect** every 5 s while disconnected or on Bluetooth only.

## Conventions
- Match the surrounding style; comments explain *why*.
- Board-specific facts go in `data/` and `docs/boards/`, never in UI code.
- Legacy and vendor material is reference only (`NOTICE.md`).
- Security audit: `docs/SECURITY_AUDIT.md`.
- Other assistants read `AGENTS.md`, which points here.
