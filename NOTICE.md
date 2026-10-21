# Licensing notice

## This project's code: GPL-3.0-or-later

Unless a path is listed below, everything in this repository was written for Aether HE and is
licensed under the [GNU General Public License, version 3 or (at your option) any later
version](LICENSE). Copyright © 2026 xpLayA and Aether HE contributors. That covers:

- the Rust workspace (`Cargo.toml`, `crates/`), including the `.slint` UI files
- `scripts/`, `packaging/`, `data/`, `fixtures/`
- `README.md`, `CLAUDE.md`, `NOTICE.md`
- `docs/SECURITY_AUDIT.md`, `docs/media/gui-*.png`, `docs/context/f75-bringup/`
- `legacy/python/tools/dump_golden_mini60.py` and the F75 additions to `legacy/python/data/` and
  `legacy/python/ui/layouts/aula-f75he.json`

Contributions are accepted under the same licence. Slint is used under its GPL-3.0-only option, so a built
`aether-gui` binary as a whole is distributed under GPLv3. This project's own source stays
GPL-3.0-or-later.

## Not covered: the original Aether-HE (MrWhosNexus)

This project started as a fork of [MrWhosNexus/Aether-HE](https://github.com/MrWhosNexus/Aether-HE).
That repository was published **without a licence**. Without a licence, the author keeps all rights,
so its material can't be relicensed here. It stays © MrWhosNexus and is included only as a reference
implementation and research record:

- `legacy/python/`, except the additions listed above
- the upstream documentation and research in `docs/context/`, `docs/superpowers/`,
  `docs/reference/design_handoff/`, and the other upstream files in `docs/`

If the original author adds a licence, this section will be updated to match.

## Not covered: vendor material (AULA / HFD)

The vendor's own files, captured from AULA's official web driver to study the protocol for
interoperability, belong to their respective owners:

- `docs/reference/driver_src/`
- `docs/context/hed-aulacn-raw/driver_src/` and `driver_deobf/`
- `docs/context/issue-6-mini60-pro-raw/driver_src/`
- the AULA product manual and screenshots under `docs/reference/design_handoff/`

## Third-party components

| Component | Licence |
|---|---|
| [Slint](https://slint.dev) UI toolkit | used under its GPL-3.0-only option (Slint is triple-licensed; attribution also shown on the Device page) |
| [Inter](https://rsms.me/inter/) typeface (`crates/aether-gui/assets/`) | SIL Open Font License 1.1 (`crates/aether-gui/assets/OFL.txt`) |
| Rust crates (hidapi, serde, clap, …) | their own licences (MIT / Apache-2.0 and similar, all GPLv3-compatible), as listed on crates.io |

## Trademarks and affiliation

AULA and other product names are trademarks of their respective owners. They are used only to say
which hardware this software works with. Aether HE is an independent project. It isn't affiliated
with or endorsed by AULA or its partners.

*This notice is a practical summary, not legal advice.*
