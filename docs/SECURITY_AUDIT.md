# Security audit — 2026-10-21

Scope: the full tracked tree at `99bee57` (362 files, upstream `MrWhosNexus/Aether-HE`), audited
before first use on a local Linux machine. **Result: no malware, backdoor, or data exfiltration found.**

## What was checked

| Area | Finding |
|---|---|
| Python network / exec (`urllib`, `socket`, `subprocess`, `ctypes`, `eval/exec`, `base64`, `pickle`) | Every call site accounted for, listed below. No obfuscated code in shipped modules. |
| `updater.py` | Talks only to `api.github.com` for the repo pinned in `version.py`. Verifies the SHA-256 and size published by GitHub before anything runs, and refuses when neither is available. Executes only on Windows (Inno installer) or Flatpak (`flatpak install`). A source checkout only downloads. **Disabled in this fork** (`version.UPDATES_ENABLED = False`). |
| `service/server.py` | Optional loopback HTTP wrapper, bound to `127.0.0.1`. Nothing starts it automatically. |
| `gamepad.py` | UAC elevation (`ShellExecuteExW runas`) is Windows-only and only runs the bundled ViGEmBus installer. On Linux it uses uinput through evdev. |
| `app_web.py` | Autostart writes an HKCU Run key on Windows only, and only when you opt in. `reveal_settings` runs `xdg-open` on the settings folder. `send_raw` (dev console) goes through the per-board NEVER-SEND guard. |
| `tools/board_bot.py`, `tools/minimax_client.py` | GitHub-Actions-only helpers (issue → board draft through the MiniMax API). Never imported by the app. |
| UI (`ui/index_runtime.html`, `ui/runtime_src/**`) | No `fetch`, XHR, WebSocket or `sendBeacon` in the shipped runtime. External URLs are only React/W3C doc strings and the repo's issues page. `localStorage` holds UI preferences only. |
| Vendored JS | `react.js` and `react-dom.js` match unpkg `react@18.3.1` UMD builds byte-for-byte (SHA-256). `tailwind.js` differs from jsDelivr `@tailwindcss/browser@4.3.0` only in minifier formatting (Terser 5.39 vs 5.48). `babel.js` is build-time only. |
| Repo plumbing | No custom git hooks. udev rules use only `uaccess` and `MODE=0660`. `run.sh`, `run.bat` and `capture.sh` (usbmon/tshark, needs sudo and is run manually) are plain. |
| `docs/context/**` scripts | Offline JSON parsers and the vendor driver bundles (reference only, never executed). |
| CI (`.github/workflows`) | `validate-submissions.yml` correctly uses `pull_request` (no secrets for untrusted code). `board-bot.yml` sends issue text to MiniMax with a repo secret. It runs on GitHub only and has no local effect. |

## Firmware-safety guards (keyboard protection)
- `protocol_mini60.DANGEROUS_CMDS = {0x4F flash, 0x0F factory reset, 0x64 calibration-on}`. These are
  refused in `drivers/mini60.py:_guard` for every frame, including the raw dev console.
- Win60: `protocol.dangerous_command_reason` (factory reset, trigger reset, blind calibration arm).
- Boards without a decoded protocol get `drivers.base.NullDriver`. Every write raises, so no board
  ever receives another board's frames.

## Key presses for reactive effects (2026-10-21)
- `aether-device/src/keys_in.rs` reads the board's own keyboard collections (usage page 1, usage 6)
  while a reactive effect (ripple, splash, …) runs, to know which key went down. It only reads, never
  writes, and only opens this board's interfaces (VID/PID match), never other keyboards.
- Only "layout key index N went down" leaves the module; usages, timing and order are not logged, stored
  or sent anywhere, and the reader stops (threads joined) when the effect stops.
- Access comes from the existing `70-aether.rules` uaccess rule (all hidraw nodes of 38A6:2729). The rule
  grants the seat user the same keystrokes the desktop already receives from that keyboard.

## Residual trust notes
- Pulling future upstream commits means trusting upstream again. Re-run this audit on large merges.
- `requirements.txt` uses lower-bound pins (`>=`). Consider a lock file for reproducible installs.
