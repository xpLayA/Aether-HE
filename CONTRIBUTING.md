# Contributing to Aether HE

Thanks for helping. Most contributions fall into one of three kinds:

| You want to… | Start here |
|---|---|
| Add support for **your keyboard** | [`docs/PORTING.md`](docs/PORTING.md) |
| Fix a bug or add a feature to the app | this file |
| Report a board without porting it | [the last section of PORTING.md](docs/PORTING.md#reporting-a-board-without-porting-it) |

AI-assisted work is welcome. [`CLAUDE.md`](CLAUDE.md) (Claude Code) and [`AGENTS.md`](AGENTS.md)
(other assistants) hold the rules your assistant has to follow. You stay responsible for what it
sends to your keyboard.

## Set up

```bash
sudo pacman -S --needed rust hidapi pkgconf   # or your distro's equivalents
./scripts/build.sh                             # dependency check, tests, release build
./target/release/aether-gui
```

Building and testing don't need a keyboard: the driver tests use a simulated board. Nothing in the
build or the tests reads `legacy/`. That folder is the original Python app, kept only as a reference.

## Branches

Work on a branch off `main`, never on `main` itself:

| Branch | For |
|---|---|
| `board/<slug>` | a keyboard port (`board/aula-mini60-he-pro`) |
| `feat/<topic>` | a new feature |
| `fix/<topic>` | a bug fix |
| `docs/<topic>` | documentation only |

Keep one board or one topic per branch, so each can be merged (or dropped) on its own. A port that's
only half-verified is still mergeable: capabilities that aren't verified stay `false`, and the UI
hides those pages.

## Commits

[Conventional commits](https://www.conventionalcommits.org), one logical step each:

```
feat(gui): …          fix(actuation): …        docs(readme): …
feat(<board-slug>): lighting verified on hardware
```

The body says *why*, and names the probe script and date for anything verified on hardware.

## Before you open a pull request

- `cargo test --workspace` passes.
- `cargo clippy --workspace` adds no new warnings.
- Hardware changes: the PR lists what was round-tripped, on which firmware and with which script. The
  script is committed under `docs/context/<board>-bringup/`.
- Docs follow along: the README's feature list and status table, and `CLAUDE.md` when protocol facts
  or rules change.

## Code map

| Path | What it is |
|---|---|
| `crates/aether-proto` | frame builders and parsers, no I/O, golden-tested |
| `crates/aether-device` | hidraw transport, `Keyboard` trait and drivers, registry, layouts, backups, profiles |
| `crates/aether-effects` | host-streamed effects (`Engine`); new effects go here |
| `crates/aether-cli` | the `aether` command |
| `crates/aether-gui` | Slint app: `ui/*.slint` layout only (via the `UiState`/`Actions` globals), `src/main.rs` glue, `src/worker.rs` owns the device |
| `data/` | boards, layouts, Fn shortcuts |
| `scripts/` | build, install, uninstall, `identify-board.py` |

## Ground rules

- Respect the [safety rules](docs/PORTING.md#safety-rules): no flash, reset or calibration commands,
  ever.
- The UI never checks a board's name. It checks capabilities.
- Contributions are licensed under GPL-3.0-or-later, the same as the project.
