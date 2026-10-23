# Instructions for AI coding assistants

This file is for assistants other than Claude Code (Codex, Cursor, Copilot, Gemini, …). Claude Code
reads `CLAUDE.md` directly.

1. **Read [`CLAUDE.md`](CLAUDE.md) first.** It holds the board-safety rules, the layout of the repo, the
   build and test commands, and the full protocol of the supported keyboard. Its rules apply to you
   too.
2. **Porting a keyboard?** Follow [`docs/PORTING.md`](docs/PORTING.md) step by step. Stop after each
   step for the user's OK.
3. **Contributing in general:** [`CONTRIBUTING.md`](CONTRIBUTING.md) covers branches, commits and checks.

## Rules that matter most

- **Never send** firmware flash, factory reset or calibration commands. On HFD boards these are `0x4F`,
  `0x0F` and `0x64`/`0x65`. Don't add them to any `Cmd` enum or probe script.
- **Ask the user before any write to a keyboard.** Probes are read-only until the user agrees. Every write
  is followed by a read-back compare and a restore in a `finally`.
- A capability in `data/boards.json` becomes `true` only after a hardware round-trip, recorded in its
  `_comment` with the date and script.
- An ack is not proof. Success is a visible effect or a byte-exact read-back.
- Board-specific facts belong in `data/`. Protocol code goes in `crates/aether-proto` (no I/O) and
  drivers go in `crates/aether-device`. Keep the UI board-agnostic: it reads capabilities.
- Run `cargo test --workspace` before you say you're done.
- Match the surrounding style. Comments explain *why*.
