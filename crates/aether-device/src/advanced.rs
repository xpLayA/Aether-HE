//! Advanced keys (DKS, MT, TGL, SOCD, RS) as edits to the key and DKS tables.
//! [`plan_advanced`] is pure: it works out the new tables, and the caller backs
//! up and writes them (DKS table first, then key records, as the vendor does).

use aether_proto::{DKS_USABLE_SLOTS, DksSlot, DksTable, KeyAction, KeyTable, SocdMode};

use crate::keys_in::usage_for_code;
use crate::{Error, Layout, Result};

/// What to put on a key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Advanced {
    /// Back to an ordinary key.
    Off,
    Toggle,
    /// Tap sends `tap`, holding longer than `delay_ms` sends `hold` (HID usages).
    ModTap { tap: u8, hold: u8, delay_ms: u16 },
    /// With `partner`: the deeper-pressed of the two wins.
    Rs { partner: usize },
    /// With `partner`: `mode` decides what both held at once sends.
    Socd { partner: usize, mode: SocdMode },
    Dks(DksSlot),
}

/// The layout name of the key sending `usage`, or the usage in hex.
pub fn usage_name(layout: &Layout, usage: u8) -> String {
    layout.keys.iter().find(|k| usage_for_code(&k.code) == Some(usage)).map_or(format!("usage {usage:#04x}"), |k| k.name.clone())
}

/// One line describing what a key does, with key names for usages.
pub fn describe(layout: &Layout, a: KeyAction, dks: &DksTable) -> String {
    let name = |u: u8| usage_name(layout, u);
    match a {
        KeyAction::Toggle => "toggle".into(),
        KeyAction::ModTap { tap, hold, delay_ms } => format!("mod-tap: tap {}, hold {} after {delay_ms} ms", name(tap), name(hold)),
        KeyAction::Rs { a, b } => format!("rapid snap {} + {}", name(a), name(b)),
        KeyAction::Socd { mode, a, b } => format!("SOCD {} + {}: {}", name(a), name(b), mode.label()),
        KeyAction::Dks { slot } => {
            let s = dks.slot(slot as usize);
            const POINTS: [&str; 4] = ["press", "bottom", "lift", "release"];
            let at = |i: usize, bit: u8| -> Vec<&str> { (0..4).filter(|&p| s.masks[p] & (bit << i) != 0).map(|p| POINTS[p]).collect() };
            let acts: Vec<String> = (0..4)
                .filter(|&i| s.actions[i] != 0)
                .map(|i| {
                    let (tap, hold) = (at(i, 1), at(i, 0x10));
                    let mut parts = Vec::new();
                    if !tap.is_empty() {
                        parts.push(format!("tap at {}", tap.join(", ")));
                    }
                    if !hold.is_empty() {
                        parts.push(format!("hold from {}", hold.join(", ")));
                    }
                    format!("{} ({})", name(s.actions[i]), parts.join("; "))
                })
                .collect();
            format!("DKS at {:.1} / {:.1} mm: {}", s.points[0] as f64 / 10.0, s.points[1] as f64 / 10.0, acts.join(", "))
        }
        KeyAction::Key { mods: 0, usage } => name(usage),
        KeyAction::None => "nothing (firmware default)".into(),
        other => format!("{other:?}"),
    }
}

/// The record `idx` goes back to when its advanced function is removed:
/// what it was in `original` (the pre-Aether table) unless that was advanced
/// too, else the key's own usage.
fn normal_record(idx: usize, layout: &Layout, original: &KeyTable) -> KeyAction {
    match original.action(idx) {
        a if !a.is_advanced() => a,
        _ => match layout.key(idx).and_then(|k| usage_for_code(&k.code)) {
            Some(usage) => KeyAction::Key { mods: 0, usage },
            None => KeyAction::None,
        },
    }
}

/// Return `idx` (and the other key of its pair, and its DKS slot) to normal.
fn clear(keys: &mut KeyTable, dks: &mut DksTable, idx: usize, layout: &Layout, original: &KeyTable) {
    if let Some(p) = keys.partner(idx) {
        keys.set(p, normal_record(p, layout, original));
    }
    if let KeyAction::Dks { slot } = keys.action(idx) {
        dks.set_slot(slot as usize, DksSlot::default());
    }
    keys.set(idx, normal_record(idx, layout, original));
}

/// The key and DKS tables with `adv` on key `idx`. `original` is the table
/// before Aether changed anything (where "off" returns a key to).
pub fn plan_advanced(
    keys: &KeyTable,
    dks: &DksTable,
    layout: &Layout,
    original: &KeyTable,
    idx: usize,
    adv: Advanced,
) -> Result<(KeyTable, DksTable)> {
    let usage = |i: usize| -> Result<u8> {
        let k = layout.key(i).ok_or_else(|| Error::Invalid(format!("index {i} is not a key on this board")))?;
        usage_for_code(&k.code).ok_or_else(|| Error::Invalid(format!("{} can't be part of a pair (it never reaches the computer)", k.name)))
    };
    usage(idx)?;
    let (mut keys, mut dks) = (keys.clone(), dks.clone());
    clear(&mut keys, &mut dks, idx, layout, original);
    match adv {
        Advanced::Off => {}
        Advanced::Toggle => keys.set(idx, KeyAction::Toggle),
        Advanced::ModTap { tap, hold, delay_ms } => {
            if !(10..=1000).contains(&delay_ms) {
                return Err(Error::Invalid(format!("hold time {delay_ms} ms is outside 10..1000 ms")));
            }
            keys.set(idx, KeyAction::ModTap { tap, hold, delay_ms });
        }
        Advanced::Rs { partner } | Advanced::Socd { partner, .. } => {
            if partner == idx {
                return Err(Error::Invalid("pick two different keys".into()));
            }
            let (a, b) = (usage(idx)?, usage(partner)?);
            clear(&mut keys, &mut dks, partner, layout, original);
            let rec = match adv {
                Advanced::Socd { mode, .. } => KeyAction::Socd { mode, a, b },
                _ => KeyAction::Rs { a, b },
            };
            keys.set(idx, rec);
            keys.set(partner, rec);
        }
        Advanced::Dks(slot) => {
            if slot.actions.iter().all(|&a| a == 0) || slot.masks.iter().all(|&m| m == 0) {
                return Err(Error::Invalid("DKS needs at least one key on at least one trigger point".into()));
            }
            if !(slot.points[0] < slot.points[1]) {
                return Err(Error::Invalid("the bottom-out point must be deeper than the press point".into()));
            }
            let used = keys.dks_slots_in_use();
            let free = (0..DKS_USABLE_SLOTS)
                .find(|s| !used.contains(s))
                .ok_or_else(|| Error::Invalid("all DKS slots are in use".into()))?;
            dks.set_slot(free, slot);
            keys.set(idx, KeyAction::Dks { slot: free as u8 });
        }
    }
    Ok((keys, dks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hfd::tests::{FakeBoard, f75, factory_keys};
    use crate::Keyboard;

    fn setup() -> (KeyTable, DksTable, Layout) {
        let kb = f75(FakeBoard::new());
        (KeyTable::from_slice(&factory_keys()).unwrap(), DksTable::default(), kb.layout().clone())
    }

    #[test]
    fn mt_and_off_restore_the_factory_record() {
        let (k, d, l) = setup();
        let (k2, _) = plan_advanced(&k, &d, &l, &k, 48, Advanced::ModTap { tap: 0x39, hold: 0xE0, delay_ms: 200 }).unwrap();
        assert_eq!(k2.action(48), KeyAction::ModTap { tap: 0x39, hold: 0xE0, delay_ms: 200 });
        let (k3, _) = plan_advanced(&k2, &d, &l, &k, 48, Advanced::Off).unwrap();
        assert_eq!(k3, k, "back to the factory table exactly");
        assert!(plan_advanced(&k, &d, &l, &k, 48, Advanced::ModTap { tap: 1, hold: 2, delay_ms: 5 }).is_err());
    }

    #[test]
    fn pairs_are_mirrored_and_cleared_together() {
        let (k, d, l) = setup();
        let (k2, _) = plan_advanced(&k, &d, &l, &k, 49, Advanced::Socd { partner: 51, mode: SocdMode::LastInput }).unwrap();
        let rec = KeyAction::Socd { mode: SocdMode::LastInput, a: 0x04, b: 0x07 };
        assert_eq!((k2.action(49), k2.action(51)), (rec, rec));
        // re-pairing A with S drops the A+D pair, D goes back to normal
        let (k3, _) = plan_advanced(&k2, &d, &l, &k, 49, Advanced::Rs { partner: 50 }).unwrap();
        assert_eq!(k3.action(51), k.action(51));
        assert_eq!(k3.partner(49), Some(50));
        let (k4, _) = plan_advanced(&k3, &d, &l, &k, 50, Advanced::Off).unwrap();
        assert_eq!(k4, k, "clearing either key clears the pair");
        assert!(plan_advanced(&k, &d, &l, &k, 49, Advanced::Rs { partner: 49 }).is_err());
    }

    #[test]
    fn dks_takes_a_free_slot_and_gives_it_back() {
        let (k, d, l) = setup();
        let s = DksSlot::new(1.0, 3.0, [0x1A, 0, 0, 0], [1, 0, 0, 1]);
        let (k2, d2) = plan_advanced(&k, &d, &l, &k, 34, Advanced::Dks(s)).unwrap();
        let (k3, d3) = plan_advanced(&k2, &d2, &l, &k, 35, Advanced::Dks(s)).unwrap();
        assert_eq!((k3.action(34), k3.action(35)), (KeyAction::Dks { slot: 0 }, KeyAction::Dks { slot: 1 }));
        assert_eq!(d3.slot(1), s);
        let (k4, d4) = plan_advanced(&k3, &d3, &l, &k, 34, Advanced::Off).unwrap();
        assert_eq!(d4.slot(0), DksSlot::default());
        assert_eq!(k4.action(34), k.action(34));
        let empty = DksSlot::new(1.0, 3.0, [0; 4], [1, 0, 0, 0]);
        assert!(plan_advanced(&k, &d, &l, &k, 34, Advanced::Dks(empty)).is_err());
    }

    #[test]
    fn fn_key_cannot_be_paired() {
        let (k, d, l) = setup();
        let fn_key = l.keys.iter().find(|x| x.code == "KeyFn").unwrap().index;
        assert!(plan_advanced(&k, &d, &l, &k, 49, Advanced::Rs { partner: fn_key }).is_err());
    }

    #[test]
    fn tables_round_trip_through_the_driver() {
        let mut kb = f75(FakeBoard::new());
        let (k, d, l) = setup();
        assert_eq!(kb.key_table().unwrap(), k);
        let (k2, d2) = plan_advanced(&k, &d, &l, &k, 34, Advanced::Dks(DksSlot::new(1.0, 3.0, [0x1A, 0, 0, 0], [1, 1, 1, 1]))).unwrap();
        kb.write_dks_table(&d2).unwrap();
        kb.write_key_table(&k2).unwrap();
        assert_eq!((kb.key_table().unwrap(), kb.dks_table().unwrap()), (k2, d2));
    }
}
