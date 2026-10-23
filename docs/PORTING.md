# Porting Aether to your keyboard

Aether's UI, profiles, backups, effects and CLI don't know which keyboard they're talking to. Everything
board-specific lives in three places:

| Where | What | You touch it when |
|---|---|---|
| `data/boards.json` | identity (VID/PID, usage page, links), protocol, **capabilities**, lighting modes, actuation range | always |
| `data/layouts/<board>.json` | every key: firmware index, name, browser code, HID usage, position and size | always |
| `data/shortcuts/<slug>.json` | the Fn combinations from the paper manual (optional, shown on the *Fn shortcuts* page) | if you have the manual |
| `crates/aether-device/src/<protocol>.rs` | a driver implementing the `Keyboard` trait | only for a new protocol family |

The GUI shows only the pages your board's capabilities allow. Lighting, Effects, Actuation and
Advanced keys appear once you set the matching capability to `true`, so a half-finished port still
looks finished.

> [!IMPORTANT]
> A capability may be `true` only after it was **round-tripped on real hardware**: written, read
> back byte-exact, restored. Until then it stays `false`, and Aether never sends those commands.

---

## 0. Before you start

- Make a branch: `git switch -c board/<slug>`, for example `board/aula-mini60-he-pro`. One board per branch.
- Close the vendor's web driver or app. It holds the device, and writes look accepted while nothing
  happens.
- Never send firmware flash, factory reset or calibration commands, even "just to see". See
  [Safety rules](#safety-rules).

## 1. Identify the board (read-only)

```bash
scripts/identify-board.py              # every HID interface
scripts/identify-board.py aula --dump  # filter by name, include the raw report descriptor
```

Look for the interface with a **vendor usage page** (`0xFF00` and up). That's where the settings
protocol lives. Note its VID:PID, interface number, usage page, and report size (64 bytes on the F75
cable, 32 on its 2.4 GHz receiver). Save the output in `fixtures/<board>/identify.txt`.

## 2. Is it a known protocol family?

| Family | Signs | Boards so far |
|---|---|---|
| **HFD** (`"protocol": "hfd"`) | vendor page `0xFF68` (cable) / `0xFF60` (receiver), no report ID, frames start `AA <cmd>`, replies start `55 <cmd>` | AULA F75 HE (verified), MINI 60 HE PRO (legacy Python, verified there) |

Many AULA Hall-effect boards share the HFD controller. If yours matches, **you probably need no
Rust code at all**: add the data files (step 3), then verify each capability (step 4).

A different protocol needs a driver (step 5). The protocol notes in `docs/context/` (vendor JS
decodes, SayoDevice captures for the WIN60 HE PRO, a multi-board taxonomy) are the place to start.

## 3. Add the data files

**`data/boards.json`**: copy the F75 entry, then change the following and set **every capability to
`false`**:
- `slug`, `name`, `vid`, `pid`, `usage_page`, `protocol`, `layout`;
- `lighting.modes` (byte values and names, from the vendor driver), `brightness_max`, `speed_max`;
- `actuation.travel_mm` / `rt_mm`;
- `links` (a 2.4 GHz receiver: its VID/PID, usage page, `frame_len`, `product` string);
- `bluetooth` (the BLE name prefix), and `wireless_capabilities` (all `false` too).

Numbers are decimal in JSON (`0x38A6` → `14502`).

**`data/layouts/<board>.json`**: `{"keys": [...]}`, one entry per physical key:

```json
{ "index": "34", "name": "W", "code": "KeyW", "hidCode": "1A", "x": "128", "y": "130", "width": "50", "height": "50" }
```

- `index` is the firmware's key index, the position in the board's tables (below 126). The vendor
  driver's layout array is the usual source.
- `x`/`y`/`width`/`height` are on any pixel grid (the F75 uses 50 px keys); the UI scales it.
- `code` is the browser `KeyboardEvent.code`, `hidCode` the USB HID usage in hex.

**`data/shortcuts/<slug>.json`** (optional): copy `aula-f75-he.json` and transcribe your manual.

Then run:

```bash
cargo test -p aether-device every_shipped_board   # checks your entry and layout
./target/release/aether info                      # does Aether find and identify the board?
```

With all capabilities `false`, the app connects, shows the board, its layout, battery, backups and Fn
shortcuts, and sends nothing but the identity read.

## 4. Verify one capability at a time

For each capability, in this order: `lighting` → `actuation` → `rapid_trigger` → `per_key_store` →
`advanced_keys` → `host_effects`:

1. **Read-only probe first.** Copy `docs/context/f75-bringup/f75_rf_probe.py`: it asserts that only
   `0x1X` (read) commands can leave. Dump the table, then save it in `fixtures/<board>/`.
2. **Round-trip.** Copy the matching `f75_*_rt.py` script, then work through the cycle:
   1. Save the current block.
   2. Write one visible change and read it back.
   3. Restore the saved block **in a `finally`** and compare it byte-exact.
   4. Watch the keyboard: an ack alone proves nothing (today's 2.4 GHz effects test was acked and
      showed nothing).
3. Flip the capability to `true` in `boards.json`. In the `_comment`, record the date and the script
   that proved it.
4. Try it in the GUI, then commit (`feat(<slug>): lighting verified`).

Wireless links get their own pass. Set `wireless_capabilities` from a run over the receiver: some
commands are dropped there, like the F75's live effect stream.

## 5. A new protocol family (Rust)

1. **Frames.** Put the frame builders and parsers in `crates/aether-proto` (no I/O). Give the new
   family its own `Cmd` enum containing only verified commands. Dangerous commands must not be
   representable, and `guard()`-style checks refuse them at the wire.
2. **Driver.** Write `crates/aether-device/src/<protocol>.rs` implementing `Keyboard`
   (`crates/aether-device/src/lib.rs`). Use `Hfd` (`hfd.rs`) as the template:
   - every write is followed by a read-back compare;
   - `require(capability, …)` gates each method;
   - the fake board in its tests shows how to test without hardware.
3. **Registry.** Register the protocol name in `open()` (`lib.rs`) and in the list in
   `every_shipped_board_is_consistent` (`data.rs`).
4. **Tests.** Add golden frame tests against captures (`crates/aether-proto/tests/`) and fake-board
   driver tests.

The `Keyboard` trait works on 126-slot tables (`KEY_SLOTS`). A board with a different internal size
maps to and from that in its driver.

## 6. Open a pull request

Use the PR template. It asks which capabilities you verified, with which script, and on which
firmware. Keep hardware probe scripts in `docs/context/<board>-bringup/`.

---

## Working with an AI assistant

The repo is set up for it:
- [`CLAUDE.md`](../CLAUDE.md) holds the safety rules, the architecture and the full HFD protocol, and
  is loaded automatically by Claude Code.
- [`AGENTS.md`](../AGENTS.md) points other assistants (Codex, Cursor, Copilot, Gemini) to the same
  rules.

A prompt that works well to start a port:

```text
I want to port Aether HE to my <brand model> (<switch type>, <size>). Read CLAUDE.md and
docs/PORTING.md first. Here is the output of scripts/identify-board.py <filter> --dump:
<paste>

1. Tell me which protocol family it looks like and why.
2. Draft the boards.json entry (all capabilities false) and the layout file from <vendor driver
   files / photo of the board>.
3. Write a READ-ONLY probe for the identity and lighting tables, in the style of
   docs/context/f75-bringup/f75_rf_probe.py, that can only send read commands.
Don't write to the keyboard until I say so, and stop after each step.
```

Things the assistant must not do without your OK on each one: send any write, flip a capability to
`true`, or touch the vendor's flash, reset or calibration commands. `CLAUDE.md` already says this. Still,
read what a probe sends before you run it.

## Safety rules

- Flash (`0x4F`), factory reset (`0x0F`) and calibration (`0x64`/`0x65`) are never sent. In a new
  protocol, find the equivalents first and make them unrepresentable.
- Take a backup before any write; Aether does it automatically (`Backup::snapshot`).
- Read-only first; every write has a restore in `finally`; on/off command pairs always both go out.
- Judge success by a visible effect or a byte-exact read-back, never by an ack alone.
- One unknown command at a time, with the vendor's app closed.

## Reporting a board without porting it

Open an issue with:
- the output of `scripts/identify-board.py <name> --dump`;
- photos of the board and its manual's Fn page;
- the vendor driver's URL.

That's enough for someone else to start a port.
