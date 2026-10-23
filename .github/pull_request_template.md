## What and why

<!-- One or two sentences. -->

## Kind
- [ ] Keyboard port (`board/<slug>`)
- [ ] Feature
- [ ] Fix
- [ ] Docs

## Hardware (ports and anything that writes to a keyboard)
| Capability | Wired | 2.4 GHz | Script / date |
|---|---|---|---|
| lighting | | | |
| actuation | | | |
| rapid_trigger | | | |
| per_key_store | | | |
| advanced_keys | | | |
| host_effects | | | |

Board and firmware: <!-- e.g. AULA F75 HE, V1.55 -->

## Checklist
- [ ] `cargo test --workspace` passes
- [ ] No flash / factory reset / calibration commands added anywhere
- [ ] Every new write is read back and compared; probes restore in a `finally`
- [ ] Capabilities set to `true` only for what was round-tripped (dated `_comment` in `boards.json`)
- [ ] README / CLAUDE.md updated if features or protocol facts changed
