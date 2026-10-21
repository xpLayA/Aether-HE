//! Key records (0x12 read / 0x22 write) and the DKS table (0x18 / 0x28):
//! what each key does, including the advanced types DKS, MT, TGL, SOCD and RS.
//! Ported from `protocol_mini60.py`; all record layouts were captured from
//! the vendor driver and round-tripped on the F75 (2026-10-21).

use crate::{Cmd, Frame, KEY_SLOTS, KEY_TABLE_SIZE, ProtoError, paged_reads, paged_writes};

pub const KEY_RECORD_SIZE: usize = 4;
/// The vendor writes records 0..125 only; the 8-byte tail that 0x12 reads is never written.
pub const KEY_WRITE_SIZE: usize = KEY_SLOTS * KEY_RECORD_SIZE; // 504
pub const DKS_SLOT_SIZE: usize = 16;
pub const DKS_SLOTS: usize = 64;
pub const DKS_TABLE_SIZE: usize = DKS_SLOTS * DKS_SLOT_SIZE; // 1024
/// The vendor allocates the first free slot below 63; 63 is never used.
pub const DKS_USABLE_SLOTS: usize = 63;
/// DKS travel points are in 0.1 mm (the actuation table uses 0.01 mm).
pub const DKS_UNIT_MM: f64 = 0.1;
/// Mod-tap hold time: one byte in 10 ms units (captured: 250 ms -> 25, 400 ms -> 40).
pub const MT_UNIT_MS: u16 = 10;

/// SOCD behaviour when both keys are held (vendor `SOCDOptions` values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocdMode {
    LastInput = 3,
    Key1 = 1,
    Key2 = 2,
    Neutral = 4,
}

impl SocdMode {
    pub const ALL: [SocdMode; 4] = [SocdMode::LastInput, SocdMode::Key1, SocdMode::Key2, SocdMode::Neutral];

    pub fn from_byte(b: u8) -> Option<SocdMode> {
        SocdMode::ALL.into_iter().find(|m| *m as u8 == b)
    }

    pub fn label(self) -> &'static str {
        match self {
            SocdMode::LastInput => "Last input wins",
            SocdMode::Key1 => "Key 1 wins",
            SocdMode::Key2 => "Key 2 wins",
            SocdMode::Neutral => "Neutral (neither)",
        }
    }
}

/// One key's 4-byte record. HID usages are keyboard-page (0x07) codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// `00 00 00 00`: nothing bound (the F75's F1-F12 base records read this way).
    None,
    /// `02 mods usage 00`: an ordinary key, optionally with modifiers held.
    Key { mods: u8, usage: u8 },
    /// `08 slot 00 00`: dynamic keystroke, configured in DKS table slot `slot`.
    Dks { slot: u8 },
    /// `09 tap hold delay`: tap for one key, hold past `delay_ms` for another.
    ModTap { tap: u8, hold: u8, delay_ms: u16 },
    /// `0A 00 00 00`: press once to latch the key down, again to release.
    Toggle,
    /// `0B mode a b` on both keys of the pair.
    Socd { mode: SocdMode, a: u8, b: u8 },
    /// `0C 00 a b` on both keys: of the two, the deeper-pressed key wins.
    Rs { a: u8, b: u8 },
    /// Anything else (consumer, mouse, macro, …), kept byte-exact.
    Raw([u8; KEY_RECORD_SIZE]),
}

impl KeyAction {
    pub fn from_bytes(r: [u8; KEY_RECORD_SIZE]) -> KeyAction {
        match r {
            [0, 0, 0, 0] => KeyAction::None,
            [2, mods, usage, 0] => KeyAction::Key { mods, usage },
            [8, slot, 0, 0] if (slot as usize) < DKS_SLOTS => KeyAction::Dks { slot },
            [9, tap, hold, d] => KeyAction::ModTap { tap, hold, delay_ms: d as u16 * MT_UNIT_MS },
            [0x0A, 0, 0, 0] => KeyAction::Toggle,
            [0x0B, m, a, b] if SocdMode::from_byte(m).is_some() => {
                KeyAction::Socd { mode: SocdMode::from_byte(m).unwrap_or(SocdMode::LastInput), a, b }
            }
            [0x0C, 0, a, b] => KeyAction::Rs { a, b },
            raw => KeyAction::Raw(raw),
        }
    }

    pub fn to_bytes(self) -> [u8; KEY_RECORD_SIZE] {
        match self {
            KeyAction::None => [0; 4],
            KeyAction::Key { mods, usage } => [2, mods, usage, 0],
            KeyAction::Dks { slot } => [8, slot, 0, 0],
            KeyAction::ModTap { tap, hold, delay_ms } => {
                [9, tap, hold, (delay_ms / MT_UNIT_MS).min(0xFF) as u8]
            }
            KeyAction::Toggle => [0x0A, 0, 0, 0],
            KeyAction::Socd { mode, a, b } => [0x0B, mode as u8, a, b],
            KeyAction::Rs { a, b } => [0x0C, 0, a, b],
            KeyAction::Raw(r) => r,
        }
    }

    /// DKS, MT, TGL, SOCD or RS: one of the vendor's "advanced key" types.
    pub fn is_advanced(self) -> bool {
        matches!(self, KeyAction::Dks { .. } | KeyAction::ModTap { .. } | KeyAction::Toggle | KeyAction::Socd { .. } | KeyAction::Rs { .. })
    }

    /// Short tag for the on-screen keycap.
    pub fn badge(self) -> &'static str {
        match self {
            KeyAction::Dks { .. } => "DKS",
            KeyAction::ModTap { .. } => "MT",
            KeyAction::Toggle => "TGL",
            KeyAction::Socd { .. } => "SOCD",
            KeyAction::Rs { .. } => "RS",
            _ => "",
        }
    }
}

/// The whole 0x12 table, raw, so records Aether doesn't touch are written back exactly.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyTable(pub [u8; KEY_TABLE_SIZE]);

impl std::fmt::Debug for KeyTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let adv = (0..KEY_SLOTS).filter(|&i| self.action(i).is_advanced()).count();
        write!(f, "KeyTable({adv} advanced keys)")
    }
}

impl KeyTable {
    pub fn from_slice(b: &[u8]) -> Result<KeyTable, ProtoError> {
        let arr = b.try_into().map_err(|_| ProtoError::BadLength { expected: KEY_TABLE_SIZE, got: b.len() })?;
        Ok(KeyTable(arr))
    }

    pub fn action(&self, idx: usize) -> KeyAction {
        let o = idx * KEY_RECORD_SIZE;
        KeyAction::from_bytes([self.0[o], self.0[o + 1], self.0[o + 2], self.0[o + 3]])
    }

    pub fn set(&mut self, idx: usize, a: KeyAction) {
        if idx < KEY_SLOTS {
            let o = idx * KEY_RECORD_SIZE;
            self.0[o..o + KEY_RECORD_SIZE].copy_from_slice(&a.to_bytes());
        }
    }

    /// The key paired with `idx` by an SOCD or RS record (the other key
    /// holding the identical record).
    pub fn partner(&self, idx: usize) -> Option<usize> {
        let a = self.action(idx);
        if !matches!(a, KeyAction::Socd { .. } | KeyAction::Rs { .. }) {
            return None;
        }
        (0..KEY_SLOTS).find(|&i| i != idx && self.action(i) == a)
    }

    /// DKS slots referenced by key records.
    pub fn dks_slots_in_use(&self) -> Vec<usize> {
        (0..KEY_SLOTS)
            .filter_map(|i| match self.action(i) {
                KeyAction::Dks { slot } => Some(slot as usize),
                _ => None,
            })
            .collect()
    }

    /// The 0x22 pass (9 frames, records 0..125). Refuses a table whose tail
    /// holds data, since the write can't carry it.
    pub fn write_frames(&self) -> Result<Vec<Frame>, ProtoError> {
        if self.0[KEY_WRITE_SIZE..].iter().any(|&b| b != 0) {
            return Err(ProtoError::BadLength { expected: KEY_WRITE_SIZE, got: KEY_TABLE_SIZE });
        }
        Ok(paged_writes(Cmd::KeyTableWrite, &self.0[..KEY_WRITE_SIZE]))
    }
}

/// One DKS slot: four travel points and up to four keys, each with a tap
/// and a hold bit per point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DksSlot {
    /// press, bottom-out, release-from-bottom, release, in 0.1 mm.
    pub points: [u8; 4],
    /// HID usages of actions 1..4 (0 = unused).
    pub actions: [u8; 4],
    /// Per point: low nibble = tap bits for actions 1..4, high nibble = hold bits.
    pub masks: [u8; 4],
}

impl DksSlot {
    /// The vendor's two-depth form: press at `press_mm`, bottom out at
    /// `bottom_mm`, with the release points mirroring them.
    pub fn new(press_mm: f64, bottom_mm: f64, actions: [u8; 4], masks: [u8; 4]) -> DksSlot {
        let raw = |mm: f64| (mm / DKS_UNIT_MM).round().clamp(1.0, 255.0) as u8;
        let (p, b) = (raw(press_mm), raw(bottom_mm));
        DksSlot { points: [p, b, b, p], actions, masks }
    }

    pub fn from_bytes(b: &[u8]) -> DksSlot {
        DksSlot {
            points: [b[0], b[1], b[2], b[3]],
            actions: [b[5], b[7], b[9], b[11]],
            masks: [b[12], b[13], b[14], b[15]],
        }
    }

    pub fn to_bytes(self) -> [u8; DKS_SLOT_SIZE] {
        let mut s = [0u8; DKS_SLOT_SIZE];
        s[..4].copy_from_slice(&self.points);
        for (i, a) in self.actions.iter().enumerate() {
            s[5 + 2 * i] = *a;
        }
        s[12..].copy_from_slice(&self.masks);
        s
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DksTable(pub [u8; DKS_TABLE_SIZE]);

impl Default for DksTable {
    fn default() -> Self {
        DksTable([0; DKS_TABLE_SIZE])
    }
}

impl std::fmt::Debug for DksTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let used = (0..DKS_SLOTS).filter(|&i| self.slot(i) != DksSlot::default()).count();
        write!(f, "DksTable({used} slots used)")
    }
}

impl DksTable {
    pub fn from_slice(b: &[u8]) -> Result<DksTable, ProtoError> {
        let arr = b.try_into().map_err(|_| ProtoError::BadLength { expected: DKS_TABLE_SIZE, got: b.len() })?;
        Ok(DksTable(arr))
    }

    pub fn slot(&self, i: usize) -> DksSlot {
        DksSlot::from_bytes(&self.0[i * DKS_SLOT_SIZE..(i + 1) * DKS_SLOT_SIZE])
    }

    pub fn set_slot(&mut self, i: usize, s: DksSlot) {
        self.0[i * DKS_SLOT_SIZE..(i + 1) * DKS_SLOT_SIZE].copy_from_slice(&s.to_bytes());
    }

    pub fn write_frames(&self) -> Vec<Frame> {
        paged_writes(Cmd::DksWrite, &self.0)
    }
}

pub fn dks_reads() -> Vec<Frame> {
    paged_reads(Cmd::DksRead, DKS_TABLE_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{REPLY_MAGIC, assemble, guard};

    #[test]
    fn records_match_the_captured_and_probed_bytes() {
        // vendor captures (MINI 60) and the F75 probe (fixtures/f75/README.md)
        for (bytes, action) in [
            ([0x02, 0x00, 0x29, 0x00], KeyAction::Key { mods: 0, usage: 0x29 }),
            ([0x08, 0x01, 0x00, 0x00], KeyAction::Dks { slot: 1 }),
            ([0x09, 0x00, 0x00, 0x19], KeyAction::ModTap { tap: 0, hold: 0, delay_ms: 250 }),
            ([0x09, 0x45, 0x44, 0x19], KeyAction::ModTap { tap: 0x45, hold: 0x44, delay_ms: 250 }),
            ([0x0A, 0x00, 0x00, 0x00], KeyAction::Toggle),
            ([0x0B, 0x03, 0x04, 0x07], KeyAction::Socd { mode: SocdMode::LastInput, a: 0x04, b: 0x07 }),
            ([0x0B, 0x02, 0x31, 0x38], KeyAction::Socd { mode: SocdMode::Key2, a: 0x31, b: 0x38 }),
            ([0x0C, 0x00, 0x14, 0x08], KeyAction::Rs { a: 0x14, b: 0x08 }),
            ([0x00, 0x00, 0x00, 0x00], KeyAction::None),
            ([0x03, 0xE2, 0x00, 0x00], KeyAction::Raw([0x03, 0xE2, 0x00, 0x00])),
        ] {
            assert_eq!(KeyAction::from_bytes(bytes), action, "{bytes:02x?}");
            assert_eq!(action.to_bytes(), bytes, "{action:?}");
        }
    }

    #[test]
    fn dks_slot_layout_matches_capture() {
        // dongle capture, slot 1: 1.6 / 3.0 / 3.0 / 1.6 mm, W tapped at every point
        let s = DksSlot::new(1.6, 3.0, [0x1A, 0, 0, 0], [1, 1, 1, 1]);
        assert_eq!(s.to_bytes(), [0x10, 0x1E, 0x1E, 0x10, 0, 0x1A, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1]);
        assert_eq!(DksSlot::from_bytes(&s.to_bytes()), s);
    }

    #[test]
    fn pairs_and_slots_are_found() {
        let mut t = KeyTable([0; KEY_TABLE_SIZE]);
        let rs = KeyAction::Rs { a: 4, b: 7 };
        t.set(49, rs);
        t.set(51, rs);
        t.set(34, KeyAction::Dks { slot: 2 });
        assert_eq!((t.partner(49), t.partner(51), t.partner(34)), (Some(51), Some(49), None));
        assert_eq!(t.dks_slots_in_use(), [2]);
    }

    #[test]
    fn writes_cover_504_bytes_and_refuse_a_dirty_tail() {
        let mut t = KeyTable([0; KEY_TABLE_SIZE]);
        t.set(0, KeyAction::Key { mods: 0, usage: 0x29 });
        let frames = t.write_frames().unwrap();
        assert_eq!(frames.len(), 9);
        let mut back = [0u8; KEY_TABLE_SIZE];
        for f in &frames {
            assert_eq!(guard(f), Ok(Cmd::KeyTableWrite));
            let mut r = *f;
            r[0] = REPLY_MAGIC;
            assemble(&mut back, &r);
        }
        assert_eq!(back, t.0);
        t.0[KEY_TABLE_SIZE - 1] = 1;
        assert!(t.write_frames().is_err());

        let d = DksTable::default().write_frames();
        assert_eq!((d.len(), d.last().map(|f| (f[2], f[6]))), (19, Some((16, 1))), "18 x 56 + 16-byte tail");
        assert!(d.iter().all(|f| guard(f) == Ok(Cmd::DksWrite)));
    }
}
