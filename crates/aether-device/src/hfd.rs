//! Driver for the HFD protocol family (AULA F75 HE; same wire format as the
//! MINI 60 HE PRO). Port of `legacy/python/drivers/mini60.py`, limited to what
//! has been verified on the F75: lighting, host stream, actuation and RT.

use std::thread::sleep;
use std::time::{Duration, Instant};

use aether_proto::{
    ACT_TABLE_SIZE, ActTable, Actuation, DKS_TABLE_SIZE, DeviceInfo, DksTable, Frame, KEY_SLOTS, KEY_TABLE_SIZE,
    KeyTable, LED_TABLE_SIZE, LEVEL_MAX, dks_reads, key_table_reads,
    LIGHT_MODE_CUSTOM, Lighting, actuation_reads, assemble, custom_led_reads, custom_led_writes, device_info_reads,
    led_table, light_read, match_reply, mm_to_raw, parse_led_table, repage, stream_clear, stream_frames,
};

use crate::{Board, Error, Keyboard, Layout, Result, Transport};

const REPLY_TIMEOUT: Duration = Duration::from_millis(500);
/// Pause between pages of a table write, as the vendor driver does.
const WRITE_GAP: Duration = Duration::from_millis(5);

pub struct Hfd<T: Transport> {
    t: T,
    board: Board,
    layout: Layout,
}

impl<T: Transport> Hfd<T> {
    pub fn new(t: T, board: Board, layout: Layout) -> Hfd<T> {
        Hfd { t, board, layout }
    }

    /// Discard stale input (acks of earlier writes) so a read sees only its reply.
    fn drain(&mut self) {
        for _ in 0..64 {
            match self.t.read(2) {
                Ok(r) if !r.is_empty() => continue,
                _ => break,
            }
        }
    }

    fn transact(&mut self, req: &Frame) -> Result<Vec<u8>> {
        // The 2.4 GHz link loses the odd request; reads and table pages are
        // both safe to repeat.
        let tries = if self.board.wireless { 2 } else { 1 };
        for _ in 0..tries {
            self.t.write(req)?;
            let deadline = Instant::now() + REPLY_TIMEOUT;
            while Instant::now() < deadline {
                let raw = self.t.read(50)?;
                if let Some(r) = match_reply(&raw, req) {
                    return Ok(r.to_vec());
                }
            }
        }
        Err(Error::Timeout { cmd: req[1], offset: req[3] as u16 | (req[4] as u16) << 8 })
    }

    /// All-or-nothing paged read: any missing page fails the whole read, so a
    /// read-modify-write can never proceed on a partial table.
    fn read_table(&mut self, frames: &[Frame], size: usize) -> Result<Vec<u8>> {
        self.drain();
        let mut table = vec![0u8; size];
        for f in &self.paged(frames) {
            let rep = self.transact(f)?;
            assemble(&mut table, &rep);
        }
        Ok(table)
    }

    /// `frames` cut to the link's report size (unchanged when wired).
    fn paged(&self, frames: &[Frame]) -> Vec<Frame> {
        repage(frames, self.t.frame_len())
    }

    /// Send a write pass. Over 2.4 GHz each page waits for its `0x55` ack (as
    /// the vendor driver does there), so a sleeping keyboard fails the write at
    /// the first page instead of silently losing some of them.
    fn write_all(&mut self, frames: &[Frame]) -> Result<()> {
        if self.board.wireless {
            self.drain();
        }
        for f in self.paged(frames) {
            if self.board.wireless {
                self.transact(&f)?;
            } else {
                self.t.write(&f)?;
                sleep(WRITE_GAP);
            }
        }
        Ok(())
    }

    fn require(&self, ok: bool, what: &str) -> Result<()> {
        if ok {
            Ok(())
        } else {
            Err(Error::Unsupported(format!("{what} is not verified on {}", self.board.name)))
        }
    }

    fn check_actuation(&self, indices: &[usize], setting: Actuation) -> Result<()> {
        if indices.is_empty() {
            return Err(Error::Invalid("no keys selected".into()));
        }
        if let Some(i) = indices.iter().find(|&&i| !self.layout.contains(i)) {
            return Err(Error::Invalid(format!("index {i} is not a key on {}", self.board.name)));
        }
        let lim = &self.board.actuation;
        // Compare in the 0.01 mm units the board stores, so float steps like
        // 0.1 + 330 * 0.01 = 3.4000000000000004 count as the 3.40 mm they are.
        let in_range = |v: f64, [lo, hi]: [f64; 2], what: &str| {
            if (mm_to_raw(lo)..=mm_to_raw(hi)).contains(&mm_to_raw(v)) {
                Ok(())
            } else {
                Err(Error::Invalid(format!("{what} {v:.2} mm is outside {lo:.2}..{hi:.2} mm")))
            }
        };
        match setting {
            Actuation::Default => Ok(()),
            Actuation::Fixed { travel_mm } => in_range(travel_mm, lim.travel_mm, "travel"),
            Actuation::RapidTrigger { travel_mm, press_mm, release_mm } => {
                self.require(self.board.capabilities.rapid_trigger, "rapid trigger")?;
                in_range(travel_mm, lim.travel_mm, "travel")?;
                in_range(press_mm, lim.rt_mm, "RT press")?;
                if release_mm > 0.0 {
                    in_range(release_mm, lim.rt_mm, "RT release")?;
                }
                Ok(())
            }
        }
    }
}

impl<T: Transport> Keyboard for Hfd<T> {
    fn board(&self) -> &Board {
        &self.board
    }

    fn layout(&self) -> &Layout {
        &self.layout
    }

    fn device_info(&mut self) -> Result<DeviceInfo> {
        self.drain();
        let rep = self.transact(&self.paged(&device_info_reads())[0])?;
        let info = DeviceInfo::parse(&rep).ok_or_else(|| Error::BadReply("device info".into()))?;
        // Over a receiver the board may report the receiver's identity or its own.
        let known = self.board.all_links().iter().any(|l| (l.vid, l.pid) == (info.vid, info.pid));
        if !known {
            return Err(Error::BadReply(format!(
                "board reports {:04X}:{:04X}, expected {:04X}:{:04X}",
                info.vid, info.pid, self.board.vid, self.board.pid
            )));
        }
        Ok(info)
    }

    fn lighting(&mut self) -> Result<Lighting> {
        self.require(self.board.capabilities.lighting, "lighting")?;
        self.drain();
        let rep = self.transact(&light_read())?;
        Lighting::from_reply(&rep).ok_or_else(|| Error::BadReply("lighting".into()))
    }

    fn set_lighting(&mut self, l: &Lighting) -> Result<()> {
        self.require(self.board.capabilities.lighting, "lighting")?;
        let info = &self.board.lighting;
        if info.mode(l.mode).is_none() && l.mode != LIGHT_MODE_CUSTOM {
            return Err(Error::Invalid(format!("lighting mode {} is not known for this board", l.mode)));
        }
        if l.brightness > info.brightness_max || l.speed > info.speed_max {
            return Err(Error::Invalid(format!(
                "brightness/speed must be 0..={}/0..={}",
                info.brightness_max, info.speed_max
            )));
        }
        self.write_all(&[l.write_frame()])
    }

    fn actuation_table(&mut self) -> Result<ActTable> {
        self.require(self.board.capabilities.actuation, "actuation")?;
        let raw = self.read_table(&actuation_reads(), ACT_TABLE_SIZE)?;
        Ok(ActTable::from_slice(&raw)?)
    }

    fn write_actuation_table(&mut self, table: &ActTable) -> Result<()> {
        self.require(self.board.capabilities.actuation, "actuation")?;
        self.write_all(&table.write_frames())
    }

    fn validate_actuation(&self, indices: &[usize], setting: Actuation) -> Result<()> {
        self.require(self.board.capabilities.actuation, "actuation")?;
        self.check_actuation(indices, setting)
    }

    fn set_actuation(&mut self, indices: &[usize], setting: Actuation) -> Result<()> {
        self.validate_actuation(indices, setting)?;
        let mut table = self.actuation_table()?;
        table.apply(indices, setting);
        self.write_actuation_table(&table)?;
        sleep(Duration::from_millis(50));
        let back = self.actuation_table()?;
        if back != table {
            return Err(Error::BadReply("actuation read-back does not match what was written".into()));
        }
        Ok(())
    }

    fn custom_leds(&mut self) -> Result<[[u8; 3]; KEY_SLOTS]> {
        self.require(self.board.capabilities.per_key_store, "stored per-key colours")?;
        Ok(parse_led_table(&self.read_table(&custom_led_reads(), LED_TABLE_SIZE)?))
    }

    fn set_custom_leds(&mut self, colors: &[[u8; 3]; KEY_SLOTS]) -> Result<()> {
        self.require(self.board.capabilities.per_key_store, "stored per-key colours")?;
        self.write_all(&custom_led_writes(&led_table(colors)))?;
        sleep(Duration::from_millis(50));
        if self.custom_leds()? != *colors {
            return Err(Error::BadReply("per-key colour read-back does not match what was written".into()));
        }
        Ok(())
    }

    fn key_table(&mut self) -> Result<KeyTable> {
        self.require(self.board.capabilities.advanced_keys, "key records")?;
        Ok(KeyTable::from_slice(&self.read_table(&key_table_reads(), KEY_TABLE_SIZE)?)?)
    }

    fn write_key_table(&mut self, t: &KeyTable) -> Result<()> {
        self.require(self.board.capabilities.advanced_keys, "key records")?;
        self.write_all(&t.write_frames()?)?;
        sleep(Duration::from_millis(50));
        if self.key_table()? != *t {
            return Err(Error::BadReply("key table read-back does not match what was written".into()));
        }
        Ok(())
    }

    fn dks_table(&mut self) -> Result<DksTable> {
        self.require(self.board.capabilities.advanced_keys, "DKS")?;
        Ok(DksTable::from_slice(&self.read_table(&dks_reads(), DKS_TABLE_SIZE)?)?)
    }

    fn write_dks_table(&mut self, t: &DksTable) -> Result<()> {
        self.require(self.board.capabilities.advanced_keys, "DKS")?;
        self.write_all(&t.write_frames())?;
        sleep(Duration::from_millis(50));
        if self.dks_table()? != *t {
            return Err(Error::BadReply("DKS table read-back does not match what was written".into()));
        }
        Ok(())
    }

    fn stream_begin(&mut self) -> Result<()> {
        self.require(self.board.capabilities.host_effects, "host effects")?;
        // The vendor's own custom-mode entry, as in Mini60Driver.begin_host_stream.
        let custom = Lighting {
            mode: LIGHT_MODE_CUSTOM,
            rgb: [0x90, 0xEE, 0x90],
            alpha: 0xFF,
            color_mode: 1,
            brightness: LEVEL_MAX,
            speed: LEVEL_MAX,
            reserved: [0; 3],
        };
        self.t.write(&custom.write_frame())
    }

    fn stream_frame(&mut self, colors: &[[u8; 3]; KEY_SLOTS]) -> Result<()> {
        for f in stream_frames(&led_table(colors)) {
            self.t.write(&f)?;
        }
        Ok(())
    }

    fn stream_end(&mut self) -> Result<()> {
        self.t.write(&stream_clear())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::Registry;
    use aether_proto::{Cmd, PAGE, REPLY_MAGIC, guard};
    use std::collections::VecDeque;

    /// A fake F75: answers reads from its own tables and applies writes.
    #[derive(Default)]
    pub struct FakeBoard {
        pub act: Vec<u8>,
        pub leds: Vec<u8>,
        pub keys: Vec<u8>,
        pub dks: Vec<u8>,
        pub light: [u8; 16],
        pub sent: Vec<u8>,
        pub replies: VecDeque<Vec<u8>>,
        /// Drop replies to this command (simulates a dead read).
        pub mute: Option<u8>,
    }

    impl FakeBoard {
        pub fn new() -> FakeBoard {
            FakeBoard { act: vec![0; ACT_TABLE_SIZE], leds: vec![0; LED_TABLE_SIZE], keys: factory_keys(), dks: vec![0; DKS_TABLE_SIZE], light: [11, 255, 255, 255, 0, 0, 0, 0, 1, 5, 3, 0, 0, 0, 0, 0], ..Default::default() }
        }
    }

    impl Transport for FakeBoard {
        fn write(&mut self, f: &Frame) -> Result<()> {
            let cmd = guard(f)?;
            self.sent.push(cmd.byte());
            let off = f[3] as usize | (f[4] as usize) << 8;
            let len = f[2] as usize;
            let mut rep = f.to_vec();
            rep[0] = REPLY_MAGIC;
            match cmd {
                Cmd::DeviceInfo => rep[12..16].copy_from_slice(&[0xA6, 0x38, 0x29, 0x27]),
                Cmd::LightRead => rep[8..24].copy_from_slice(&self.light),
                Cmd::ActuationRead => rep[8..8 + PAGE].copy_from_slice(&self.act[off..off + PAGE]),
                Cmd::CustomLedRead => rep[8..8 + PAGE].copy_from_slice(&self.leds[off..off + PAGE]),
                Cmd::KeyTableRead => rep[8..8 + len].copy_from_slice(&self.keys[off..off + len]),
                Cmd::DksRead => rep[8..8 + len].copy_from_slice(&self.dks[off..off + len]),
                Cmd::KeyTableWrite => {
                    self.keys[off..off + len].copy_from_slice(&f[8..8 + len]);
                    return Ok(());
                }
                Cmd::DksWrite => {
                    self.dks[off..off + len].copy_from_slice(&f[8..8 + len]);
                    return Ok(());
                }
                Cmd::CustomLedWrite => {
                    self.leds[off..off + len].copy_from_slice(&f[8..8 + len]);
                    return Ok(());
                }
                Cmd::ActuationWrite => {
                    self.act[off..off + len].copy_from_slice(&f[8..8 + len]);
                    return Ok(());
                }
                Cmd::LightWrite => {
                    self.light.copy_from_slice(&f[8..24]);
                    return Ok(());
                }
                _ => return Ok(()),
            }
            if self.mute != Some(cmd.byte()) {
                self.replies.push_back(rep);
            }
            Ok(())
        }

        fn read(&mut self, _timeout_ms: i32) -> Result<Vec<u8>> {
            Ok(self.replies.pop_front().unwrap_or_default())
        }
    }

    /// The key table read from a factory-state F75.
    pub fn factory_keys() -> Vec<u8> {
        let text = include_str!("../../../fixtures/f75/backup.json");
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let h = v["key_table"].as_str().unwrap();
        (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
    }

    pub fn f75(fake: FakeBoard) -> Hfd<FakeBoard> {
        let reg = Registry::load().unwrap();
        let board = reg.boards.into_iter().find(|b| b.slug == "aula-f75-he").unwrap();
        let layout = Layout::load(&board).unwrap();
        Hfd::new(fake, board, layout)
    }

    #[test]
    fn device_info_checks_identity() {
        let mut kb = f75(FakeBoard::new());
        let info = kb.device_info().unwrap();
        assert_eq!((info.vid, info.pid), (0x38A6, 0x2729));
    }

    #[test]
    fn set_actuation_patches_only_the_selected_key_and_verifies() {
        let mut fake = FakeBoard::new();
        fake.act[8 * 49..8 * 50].copy_from_slice(&[0, 0, 200, 0, 0, 0, 0, 0]); // A at 2.00
        let mut kb = f75(fake);
        kb.set_actuation(&[34], Actuation::RapidTrigger { travel_mm: 1.5, press_mm: 0.3, release_mm: 0.3 })
            .unwrap();
        let t = kb.actuation_table().unwrap();
        assert_eq!(t.record(34).trip, 150);
        assert!(t.record(34).rapid_trigger());
        assert_eq!(t.record(49).trip, 200, "neighbour untouched");
        assert!((0..KEY_SLOTS).filter(|&i| i != 34 && i != 49).all(|i| t.record(i).is_empty()));
    }

    #[test]
    fn failed_read_aborts_before_any_write() {
        let mut fake = FakeBoard::new();
        fake.mute = Some(Cmd::ActuationRead.byte());
        let mut kb = f75(fake);
        let err = kb.set_actuation(&[34], Actuation::Fixed { travel_mm: 1.5 }).unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }), "{err}");
        assert!(!kb.t.sent.contains(&Cmd::ActuationWrite.byte()));
    }

    #[test]
    fn invalid_actuation_is_refused_without_touching_the_board() {
        let mut kb = f75(FakeBoard::new());
        for (keys, s) in [
            (vec![], Actuation::Fixed { travel_mm: 1.0 }),
            (vec![13], Actuation::Fixed { travel_mm: 1.0 }), // slot exists, key does not
            (vec![34], Actuation::Fixed { travel_mm: 0.0 }),
            (vec![34], Actuation::Fixed { travel_mm: 0.09 }),
            (vec![34], Actuation::Fixed { travel_mm: 3.41 }),
            (vec![34], Actuation::Fixed { travel_mm: 4.0 }),
            (vec![34], Actuation::RapidTrigger { travel_mm: 1.0, press_mm: 0.0, release_mm: 0.1 }),
        ] {
            assert!(kb.set_actuation(&keys, s).is_err(), "{keys:?} {s:?}");
        }
        assert!(kb.t.sent.is_empty());
    }

    #[test]
    fn range_ends_are_accepted_despite_float_steps() {
        let mut kb = f75(FakeBoard::new());
        let top = 0.1 + 330.0 * 0.01; // what a 0.01-step slider produces at 3.40
        assert!(top > 3.4);
        kb.set_actuation(&[34], Actuation::Fixed { travel_mm: top }).unwrap();
        assert_eq!(kb.actuation_table().unwrap().record(34).trip, 340);
        kb.set_actuation(&[34], Actuation::Fixed { travel_mm: 0.1 }).unwrap();
        let rt_top = 0.01 + 339.0 * 0.01;
        kb.set_actuation(&[34], Actuation::RapidTrigger { travel_mm: top, press_mm: rt_top, release_mm: 0.01 }).unwrap();
        let r = kb.actuation_table().unwrap().record(34);
        assert_eq!((r.trip, r.press, r.release), (340, 340, 1));
    }

    #[test]
    fn custom_leds_store_and_verify() {
        let mut kb = f75(FakeBoard::new());
        let mut c = kb.custom_leds().unwrap();
        assert!(c.iter().all(|x| *x == [0; 3]));
        c[34] = [255, 0, 80];
        kb.set_custom_leds(&c).unwrap();
        assert_eq!(kb.custom_leds().unwrap()[34], [255, 0, 80]);
        assert_eq!(kb.t.leds[34 * 4], 34, "record carries its key index");
    }

    #[test]
    fn lighting_round_trips_byte_exact() {
        let mut kb = f75(FakeBoard::new());
        let before = kb.lighting().unwrap();
        let red = Lighting { mode: 1, rgb: [255, 0, 0], color_mode: 0, ..before };
        kb.set_lighting(&red).unwrap();
        assert_eq!(kb.lighting().unwrap().mode, 1);
        kb.set_lighting(&before).unwrap();
        assert_eq!(kb.lighting().unwrap(), before);
        let bad = Lighting { brightness: 9, ..before };
        assert!(kb.set_lighting(&bad).is_err());
    }
}
