<div align="center">

<img src="packaging/aether-he.png" alt="Aether HE" width="120">

# Aether HE

**Native Linux control for the AULA F75 HE Hall-effect keyboard: lighting, effects, actuation,
rapid trigger and advanced keys, over USB-C or the 2.4 GHz receiver.**

![Rust](https://img.shields.io/badge/Rust-2024-b7410e?logo=rust&logoColor=white)
![Linux](https://img.shields.io/badge/Linux-Wayland%20%7C%20X11-1f6feb?logo=linux&logoColor=white)
![UI](https://img.shields.io/badge/UI-Slint-2379f4)
![License](https://img.shields.io/badge/license-GPL--3.0--or--later-3da639)
![Tested](https://img.shields.io/badge/tested%20on-AULA%20F75%20HE%20fw%20V1.55-8957e5)

[Features](#features) · [Install](#install) · [Using it](#using-it) · [Command line](#command-line) ·
[Troubleshooting](#troubleshooting) · [Development](#development)

</div>

> [!WARNING]
> **Unofficial.** Aether HE is an independent hobby project, not affiliated with, endorsed by, or
> supported by AULA or the makers of the keyboard's controller and official driver. It was built by
> studying the protocol and testing on one keyboard, and it doesn't claim to replace the official
> software. **Use it at your own risk**: see the [Disclaimer](#disclaimer).

---

## At a glance

What works over each connection, all tested on a real F75 HE:

| | USB-C cable | 2.4 GHz receiver | Bluetooth |
|---|:---:|:---:|:---:|
| Firmware lighting (19 effects) with live preview | ✅ | ✅ | — |
| Per-key colours stored on the keyboard | ✅ | ✅ | — |
| Animated host effects (9) and reactive effects (7) | ✅ | ✗ ¹ | — |
| Actuation 0.10–3.40 mm and rapid trigger, per key | ✅ | ✅ | — |
| Advanced keys: DKS, mod-tap, toggle, SOCD, rapid snap | ✅ | ✅ | — |
| Profiles, automatic original backup, verified writes | ✅ | ✅ | — |
| Battery level | n/a | ✅ | ✅ ² |
| Typing | ✅ | ✅ | ✅ |

¹ The receiver acknowledges the live colour stream but never shows it on the keys (tested 2026-10-23).
Plug in the cable for effects; everything stored on the keyboard works wirelessly.
² The keyboard offers no settings over Bluetooth. Aether recognises it, shows the battery, and takes
over as soon as you plug in the cable or switch to 2.4 GHz.

## Highlights

- **A full Rust rewrite.** The original Python app was rewritten as a Rust workspace (protocol, device,
  effects, CLI, GUI). Its protocol code is golden-tested frame by frame against the original Python
  implementation.
- **Tested on real hardware.** Every command Aether sends was tested on an F75 HE: written, read back
  and compared, then restored byte-exact. That covers the USB cable and the 2.4 GHz receiver.
- **Safety by design.** Aether's protocol code can't express the vendor protocol's flash, factory-reset
  and calibration commands, and the USB layer refuses them a second time.
- **Your original settings, kept.** Before Aether first changes anything, it saves the keyboard's
  settings once and never overwrites that file. *Restore original* takes the keyboard back to that state.
- **A desktop app that stays out of the way.** One instance per session, background effects, start at
  login, automatic reconnect after unplug or suspend, and a native Slint UI (Fluent dark).

## New in Aether (not in the official driver)

These ideas are original to Aether HE. The official AULA web driver doesn't have them:

| Feature | What it adds |
|---|---|
| **Animated host effects** | Wave, breathe, aurora, comet, tide, twinkle, rain, striation and static, drawn by Aether and streamed to every key at ~28 fps, using your own palette, speed, brightness and direction. |
| **Reactive effects** | Ripple, splash, cross, sparks, afterglow, heatmap and lightning, lit from the keys you actually press. |
| **Effects in the background** | Effects keep running with the window closed, can start hidden at login, and resume after unplug or suspend. |
| **"When Aether quits" lighting** | Choose which firmware lighting the keyboard keeps once Aether exits. Your profile's lighting comes back on the next connect. |
| **Live lighting preview** | Every change shows on the keyboard as you make it. *Cancel preview* puts the old lighting back. |
| **On-screen effect simulation** | The virtual keyboard animates a look-alike of the selected firmware effect. |
| **Tuned built-in profiles** | Competitive, Gaming, Balanced and Typing actuation, set with one click. |
| **Auto-saving profiles** | A profile holds lighting, per-key colours, every key's actuation and an effect. Every edit is saved into it, and editing a built-in forks a custom copy. |
| **Original backup and *Restore original*** | The pre-Aether state is saved once, before the first change, and never overwritten. One click (or the uninstaller) puts it back. |
| **Verified writes** | Every change is read back from the keyboard and compared. Advanced keys turned off return to their exact previous record. |
| **Bluetooth recognition** | Shows that the keyboard is on Bluetooth with its battery level, and switches over when the cable or receiver appears. |
| **Fn shortcuts in the app** | The paper manual's Fn combinations, including the F-row for each system mode, on their own page. |
| **Native Linux app and CLI** | No browser or WebHID needed. Every setting is also scriptable with `aether`. |

The official driver still has things Aether doesn't (yet): macros, plain remapping, dead zones,
calibration, sleep timers and the music-rhythm mode. See [Not supported yet](#not-supported-yet).

---

## Features

### Profiles
- **Built-in:** Firmware default, Competitive, Gaming, Balanced and Typing. They set every key's
  actuation and leave your lighting alone.
- **Custom profiles** hold your whole setup: lighting, per-key colours, every key's actuation and an
  optional effect.
- **Always saved:** there is always an active profile, and every change is saved into it. Editing while
  a built-in is active creates a custom copy, e.g. *Gaming (custom)*.

### Firmware lighting
- All 19 built-in effects, with brightness, speed and an HSV colour picker.
- **Live preview:** each change shows on the keyboard immediately. Nothing is saved until you press
  *Apply*.
- **On-screen simulation:** the virtual keyboard animates a look-alike of the selected effect.
- **Per-key paint:** select keys and paint them. The colours are stored on the keyboard, so they stay
  without the app.

### Host effects
- **Animated:** wave, breathe, aurora, comet, tide, twinkle, rain, striation and static. Use your own
  palette, background, speed, brightness and direction. Streamed to every key at ~28 fps.
- **Reactive:** ripple, splash, cross, sparks, afterglow, heatmap and lightning. They respond to the keys
  you press (read-only; nothing is stored). Clicking keys on screen triggers them too.
- **Background:** effects keep running when you close the window; *Quit* stops them. *Start at login*
  runs Aether hidden with your profile's effect.
- **When Aether quits:** choose which firmware lighting the keyboard keeps once Aether exits.

### Actuation and rapid trigger
- Per key, 0.10–3.40 mm, with rapid trigger press and release sensitivity.
- One-click presets: Competitive, Gaming, Balanced, Typing, Default.
- **Selecting keys:** click, Shift+click, drag a box, or use quick sets (All, WASD, Letters…).
- Reset a selection or the whole board to the firmware default.

### Advanced keys
Stored on the keyboard, so they work without the app:
- **DKS:** up to four actions at four points of one keypress.
- **Mod-tap:** one action on tap, another on hold.
- **Toggle**.
- **Rapid snap:** the deeper of two keys wins.
- **SOCD:** last input, key 1, key 2 or neutral.

Removing an advanced key puts it back exactly as it was before.

### Wireless and Bluetooth
- **2.4 GHz receiver:** found like the cable. The header shows the battery level (⚡ while charging).
  When both are connected, the cable is used.
- **Bluetooth:** recognised, with the battery level shown. Typing only, because the firmware exposes
  no settings there.

### Fn shortcuts
The keyboard's built-in Fn combinations, from AULA's paper manual: connection modes and pairing,
system modes, lighting and media keys. Includes the F-row for each system mode.

### Command line
Everything is scriptable with the `aether` command. See [Command line](#command-line).

---

## Install

### 1. Requirements
Linux with a graphical session (Wayland or X11). Tested on CachyOS / Arch with Hyprland. On Arch-based
systems:

```bash
sudo pacman -S --needed rust hidapi pkgconf
```

`scripts/build.sh` checks for these and prints the exact command if something is missing.

### 2. Get the code and install

```bash
git clone https://github.com/xpLayA/aether-he.git
cd aether-he
./scripts/install.sh
```

**What `install.sh` does:**
1. Builds the release binaries. The first build takes a few minutes.
2. Installs `aether-gui` and `aether` to `~/.local/bin`, and their data to `~/.local/share/aether-he`.
3. Adds **Aether HE** to your app menu, with an icon.
4. Asks for your password **once**, to install a udev rule (`/etc/udev/rules.d/70-aether.rules`). The
   rule lets your user talk to the keyboard without `sudo`.
5. Records everything it installed, so it can be removed cleanly later.

**After installing, unplug and replug the keyboard (or the receiver) once.**

| Option | Effect |
|---|---|
| `--prefix DIR` | install somewhere other than `~/.local` |
| `--no-udev` | skip the udev rule (you'll need to install it yourself) |
| `--no-build` | install the existing build as-is |

### 3. Run it
Open **Aether HE** from your app menu, or run `aether-gui`.

> [!NOTE]
> Close the official Aula web driver (browser tab) first. While it's open, it holds the keyboard.

---

## Using it

<details open>
<summary><b>Profiles</b></summary>

- On the *Profiles* page, press **Apply** on a tile, or use the **Profile** switcher at the top right
  of any page.
- Built-ins only set actuation and keep your lighting. If you change anything while a built-in is
  active, a custom copy is created and becomes active.
- While a custom profile is active, every change is saved into it. The chip under each page title
  shows where changes go.
- **Save current setup** under *New custom profile* stores everything, including a running effect,
  under a new name.
- **More** on a tile: update it from the keyboard, attach or remove an effect, rename, duplicate or
  delete it.
</details>

<details>
<summary><b>Lighting</b></summary>

- Pick an effect, colour, brightness and speed. With **On keyboard** on, the keyboard shows the change
  live.
- **Apply** keeps it. **Cancel preview**, switching page or closing the app puts the old lighting back.
- **On screen** toggles the animated simulation on the virtual keyboard.
- **Per-key paint:** choose *Custom (per-key)*, select keys, pick a colour, then press **Paint
  selected** or **Fill all**.
</details>

<details>
<summary><b>Effects</b></summary>

- Choose an effect and colours, then press **Start effect**. **Stop effect** restores your firmware
  lighting.
- Effects need the USB cable. Over 2.4 GHz the page says so, and your stored lighting and per-key
  colours stay as they are.
</details>

<details>
<summary><b>Actuation</b></summary>

- Select keys on the keyboard: click, Shift+click, drag a box, or use *All / WASD / Letters / …*.
- Click a **preset**, or set *Actuation* (and *Rapid trigger* press/release) and press **Apply**.
- Switch *Selected keys* to **All keys** to change the whole board at once.
- **Reset selected / Reset all** go back to the firmware default. Each key shows its value in mm;
  blank means the default.
</details>

<details>
<summary><b>Advanced keys</b></summary>

- Pick a key, choose DKS, mod-tap, toggle, rapid snap or SOCD, and save it to the keyboard. Rapid
  snap and SOCD take a pair of keys.
- **Off** or **Remove** makes it an ordinary key again, exactly as it was before Aether. **Remove all** clears every advanced key.
</details>

<details>
<summary><b>Backups</b></summary>

- The keyboard's original settings are saved automatically before Aether first changes anything. Only
  that one file is kept automatically, and it is never overwritten.
- **Backup now** saves an extra copy whenever you want one.
- **Restore** any of them, or **Restore original** for the pre-Aether state. Each restore is verified
  by reading the keyboard back.
</details>

<details>
<summary><b>Wireless</b></summary>

- Plug in the 2.4 GHz receiver and switch the keyboard to 2.4 GHz: Aether finds it like the cable.
- The keyboard sleeps when idle, and only a keypress wakes it. If a change says the keyboard is
  asleep, press a key and try again.
- The battery level in the header updates every minute.
- Over Bluetooth, plug in the cable or switch to 2.4 GHz to change settings. Your settings stay on the
  keyboard when you go back to Bluetooth.
</details>

## Command line

```bash
aether info                                   # identity, firmware, connection + battery
aether light get | modes
aether light set --mode static --color ff0000 --brightness 5
aether actuation get [--keys W,A,S,D]
aether actuation set --keys W,A,S,D --travel 0.8 --rt 0.15
aether actuation set --keys all --travel 1.2
aether actuation reset [--keys all]          # back to firmware default
aether profile list | save NAME | update NAME | apply NAME | duplicate NAME NEW | rename NAME NEW | delete NAME
aether keys list | off KEY | tgl KEY | mt KEY --tap Esc --hold L-Ctrl --ms 200
aether keys rs A D | socd A D --mode last|key1|key2|neutral
aether keys dks F --press 1.0 --bottom 3.0 --act F:t--- --act G:-t--   # t tap, h hold, per point
aether effect rain --colors 00ff80,0080ff --seconds 30   # cable only; Ctrl+C stops, lighting restored
aether backup [--out FILE]
aether backups                                # oldest first
aether backups --prune                        # keep only the original, merge and delete the rest
aether restore [FILE] | aether restore --original
```

Every command that changes the keyboard makes sure the original backup exists first.

---

## Update

```bash
cd aether-he
git pull
./scripts/install.sh
```

Re-running `install.sh` also refreshes the udev rule. It asks for your password only if the rule changed.

## Uninstall

```bash
./scripts/uninstall.sh
```

It lists exactly what will be removed and asks before doing anything, including what to do with the
keyboard:

```
Keyboard settings:
  [k] keep the current lighting and actuation (default)
  [r] restore the original settings, from before Aether first changed them
```

<details>
<summary>Options</summary>

| Option | Effect |
|---|---|
| `--keep-settings` / `--restore-original` | answer the keyboard question in advance |
| `--purge` | also delete your backups and GUI settings (asks again) |
| `--yes` | don't ask (keeps the keyboard settings) |
| `--prefix DIR` | the prefix used at install time |
</details>

Your **backups are kept** unless you pass `--purge`. The keyboard stores its own settings, so
uninstalling doesn't change the keyboard unless you choose *restore original*.

## Where your data lives

| What | Path |
|---|---|
| Backups (the original, plus any you save) | `~/.local/share/aether-he/backups/` |
| Profiles | `~/.config/aether-he/profiles/` |
| GUI settings | `~/.config/aether-he/gui.json` |
| Installed app data | `~/.local/share/aether-he/data/` |
| Autostart entry | `~/.config/autostart/aether-he.desktop` |

## Troubleshooting

**"No keyboard connected"**
- Over 2.4 GHz: press a key to wake the keyboard. If you installed Aether before wireless support,
  re-run `./scripts/install.sh` so the udev rule also covers the receiver (`0c45:fefe`).
- Close the Aula web driver tab.
- Make sure the udev rule is installed (`./scripts/install.sh`), then unplug and replug the keyboard.
- Run `aether info` to see the exact error.

**"Connected over Bluetooth" but nothing can be changed:** that's the keyboard. It has no settings
channel over Bluetooth. Use the cable or the 2.4 GHz receiver.

**"Effects need the USB cable":** the 2.4 GHz receiver doesn't pass live effects to the keys. Use the
cable, or firmware lighting and per-key paint, which work wirelessly.

**A setting didn't stick:** press **Read** on the Actuation page (or run `aether actuation get`) to see
what the keyboard really has, then restore a backup if needed.

**The window doesn't open:** run `aether-gui` from a terminal to see the error. It needs a working
Wayland or X11 session with OpenGL. Opening it a second time just shows the window that's already
running.

**Hyprland and start at login:** Hyprland doesn't read autostart files, so add
`exec-once = aether-gui --background` to `hyprland.conf` instead.

## Not supported yet

- Plain key remapping, macros, dead zones, sleep timers, the music-rhythm mode.
- Calibration (deliberately never sent).
- Live key-depth display (the keyboard only exposes it through calibration mode).
- Host effects over 2.4 GHz: a limit of the receiver, not something Aether can fix.
- Any settings over Bluetooth: the keyboard doesn't offer them.

---

## Development

```bash
./scripts/build.sh            # dependency check, tests, release build
cargo test --workspace        # protocol golden tests, driver tests (simulated keyboard), effects, GUI helpers
AETHER_PAGE=actuation ./target/release/aether-gui   # open on a page
```

```
 aether-gui (Slint)      aether (CLI)
        └──────────┬──────────┘
             aether-device ── aether-effects
     transport · driver · layouts · backups · profiles
                   │
             aether-proto        frame builders/parsers, no I/O
                   │
   hidraw: USB-C (64-byte frames) · 2.4 GHz receiver (32-byte frames)
```

| Crate | What it is |
|---|---|
| `crates/aether-proto` | Wire protocol: frame builders and parsers, no I/O. Golden-tested against the original Python implementation. |
| `crates/aether-device` | USB transport, keyboard driver, layouts, backups, profiles, Bluetooth detection |
| `crates/aether-effects` | Host-streamed per-key effects, animated and reactive |
| `crates/aether-cli` | The `aether` command |
| `crates/aether-gui` | The `aether-gui` desktop app (Slint): `ui/*.slint` for layout, `src/` for logic |

**Elsewhere in the repo:**
- [`CLAUDE.md`](CLAUDE.md): engineering notes and the protocol, for contributors (and AI assistants).
- [`docs/SECURITY_AUDIT.md`](docs/SECURITY_AUDIT.md): security review of the original code.
- [`docs/context/`](docs/context/) and [`legacy/python/`](legacy/python/): protocol research and the
  original Python app, kept as the reference implementation.

---

## Disclaimer

- **Unofficial.** This project is not made, endorsed or supported by AULA or its partners. For official
  support, firmware updates and warranty questions, use AULA's own software and support channels.
- **No warranty.** This software is provided "as is", without warranty of any kind. That includes the
  implied warranties of merchantability and fitness for a particular purpose. See sections 15 and 16 of
  the [GPL](LICENSE). You are responsible for any changes it makes to your keyboard.
- **Limited testing.** It has been tested on a single AULA F75 HE (USB `38A6:2729`, firmware V1.55) on
  Linux. Other units, firmware versions or revisions may behave differently.
- **Your warranty.** Using third-party configuration software may affect your keyboard's manufacturer
  warranty. Check AULA's terms if this matters to you.
- **Protocol knowledge** comes from observing the official driver for interoperability purposes. No
  official documentation was used, and none of the vendor's code is part of the app.

## License

This project's code is licensed under the **[GNU General Public License v3.0 or later](LICENSE)**.
You can use, study, share and modify it. If you distribute it, or a modified version, you must make the
source available under the same licence.

Some files aren't covered, because they belong to others. See [NOTICE.md](NOTICE.md):
- **The original Python app** (`legacy/python/`) and the upstream research. The original author published
  them without a licence, which leaves all rights with them, so we can't relicense them. They're kept for
  reference.
- **AULA/HFD vendor files** kept for protocol research.

## Credits

- Forked from [MrWhosNexus/Aether-HE](https://github.com/MrWhosNexus/Aether-HE) (Python, AULA WIN60 HE).
  Rewritten in Rust and brought up on the F75 HE.
- UI built with [Slint](https://slint.dev), used under its GPLv3 licence option (attribution on the
  Device page).
- [Inter](https://rsms.me/inter/) typeface, SIL Open Font License (`crates/aether-gui/assets/OFL.txt`).
