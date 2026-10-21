//! Wire protocol for the HFD keyboard family (AULA F75 HE, MINI 60 HE PRO),
//! ported from `legacy/python/protocol_mini60.py` and golden-tested against it
//! byte-for-byte (`tests/golden.rs`).
//!
//! Frames are 64 bytes on the wired boards: `[0]=0xAA` request magic, `[1]` command,
//! `[2]` payload length, `[3..5]` little-endian table offset, `[6]` last-chunk flag,
//! payload from byte 8. Replies echo the header with `0x55` in byte 0.
//!
//! Safety by construction: [`Cmd`] only has variants for commands that were
//! verified on hardware. The vendor's flash (0x4F), factory-reset (0x0F) and
//! calibration (0x64) commands cannot be expressed, and [`guard`] rejects them
//! again at the wire for any raw frame.

use std::fmt;

pub const FRAME_LEN: usize = 64;
pub const PAYLOAD_START: usize = 8;
/// Payload bytes per frame: the paging stride for every table.
pub const PAGE: usize = FRAME_LEN - PAYLOAD_START;
pub const MAGIC: u8 = 0xAA;
pub const REPLY_MAGIC: u8 = 0x55;

/// Flash / factory reset / calibration-on. Never sent, under any circumstances.
pub const DANGEROUS_CMDS: [u8; 3] = [0x0F, 0x4F, 0x64];

pub const DEVICE_INFO_SIZE: usize = 0x38;
pub const CONFIG_TABLE_SIZE: usize = 0x40;
pub const KEY_TABLE_SIZE: usize = 0x200;
pub const LIGHT_BLOCK_SIZE: usize = 0x10;
/// Per-key slots in every per-key table (actuation, LED, key records).
pub const KEY_SLOTS: usize = 126;
pub const ACT_RECORD_SIZE: usize = 8;
pub const ACT_TABLE_SIZE: usize = KEY_SLOTS * ACT_RECORD_SIZE; // 0x3F0
pub const LED_RECORD_SIZE: usize = 4;
pub const LED_TABLE_SIZE: usize = KEY_SLOTS * LED_RECORD_SIZE; // 504

pub const LIGHT_MODE_OFF: u8 = 0;
pub const LIGHT_MODE_STATIC: u8 = 1;
pub const LIGHT_MODE_CUSTOM: u8 = 20;
/// Brightness and speed both run 0..=5.
pub const LEVEL_MAX: u8 = 5;

/// Length byte of the payload-less 0x36 clear (SDK transaction default).
const CLEAR_LEN: usize = 0x18;

/// Every command this crate can put on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Cmd {
    DeviceInfo = 0x10,
    ConfigRead = 0x11,
    KeyTableRead = 0x12,
    LightRead = 0x13,
    CustomLedRead = 0x14,
    ActuationRead = 0x17,
    LightWrite = 0x23,
    ActuationWrite = 0x27,
    StreamLed = 0x32,
    ClearLed = 0x36,
}

impl Cmd {
    pub const ALL: [Cmd; 10] = [
        Cmd::DeviceInfo,
        Cmd::ConfigRead,
        Cmd::KeyTableRead,
        Cmd::LightRead,
        Cmd::CustomLedRead,
        Cmd::ActuationRead,
        Cmd::LightWrite,
        Cmd::ActuationWrite,
        Cmd::StreamLed,
        Cmd::ClearLed,
    ];

    pub fn byte(self) -> u8 {
        self as u8
    }

    pub fn from_byte(b: u8) -> Option<Cmd> {
        Cmd::ALL.into_iter().find(|c| c.byte() == b)
    }

    /// GET commands (0x1X) only read state; everything else changes the board.
    pub fn is_read(self) -> bool {
        self.byte() & 0xF0 == 0x10
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// A vendor NEVER-SEND command (flash / factory reset / calibration).
    Refused(u8),
    /// A command byte this crate does not know, so it will not send it.
    Unknown(u8),
    BadMagic(u8),
    BadLength { expected: usize, got: usize },
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::Refused(c) => write!(
                f,
                "refusing command 0x{c:02X}: flash/factory-reset/calibration family is never sent"
            ),
            ProtoError::Unknown(c) => write!(f, "refusing unknown command 0x{c:02X}"),
            ProtoError::BadMagic(m) => write!(f, "frame magic 0x{m:02X} is not 0xAA"),
            ProtoError::BadLength { expected, got } => {
                write!(f, "expected {expected} bytes, got {got}")
            }
        }
    }
}

impl std::error::Error for ProtoError {}

pub type Frame = [u8; FRAME_LEN];

/// Final check before any frame reaches the wire. Every transport calls this.
pub fn guard(frame: &[u8]) -> Result<Cmd, ProtoError> {
    if frame.len() != FRAME_LEN {
        return Err(ProtoError::BadLength { expected: FRAME_LEN, got: frame.len() });
    }
    if frame[0] != MAGIC {
        return Err(ProtoError::BadMagic(frame[0]));
    }
    let c = frame[1];
    if DANGEROUS_CMDS.contains(&c) {
        return Err(ProtoError::Refused(c));
    }
    Cmd::from_byte(c).ok_or(ProtoError::Unknown(c))
}

/// hidapi output report: report id 0 followed by the frame (these frames carry no id).
pub fn to_report(frame: &Frame) -> [u8; FRAME_LEN + 1] {
    let mut r = [0u8; FRAME_LEN + 1];
    r[1..].copy_from_slice(frame);
    r
}

fn header(cmd: Cmd, len: usize, offset: usize, last: bool) -> Frame {
    assert!(len <= PAGE, "chunk length {len:#x} exceeds the frame payload");
    assert!(offset <= 0xFFFF, "table offset {offset:#x} exceeds the 16-bit field");
    let mut f = [0u8; FRAME_LEN];
    f[0] = MAGIC;
    f[1] = cmd.byte();
    f[2] = len as u8;
    f[3] = (offset & 0xFF) as u8;
    f[4] = (offset >> 8) as u8;
    f[6] = last as u8;
    f
}

fn paged_reads(cmd: Cmd, size: usize) -> Vec<Frame> {
    (0..size)
        .step_by(PAGE)
        .map(|off| {
            let len = PAGE.min(size - off);
            header(cmd, len, off, off + len >= size)
        })
        .collect()
}

fn paged_writes(cmd: Cmd, data: &[u8]) -> Vec<Frame> {
    (0..data.len())
        .step_by(PAGE)
        .map(|off| {
            let len = PAGE.min(data.len() - off);
            let mut f = header(cmd, len, off, off + len >= data.len());
            f[PAYLOAD_START..PAYLOAD_START + len].copy_from_slice(&data[off..off + len]);
            f
        })
        .collect()
}

// ---------------------------------------------------------------- reads ----

pub fn device_info_reads() -> Vec<Frame> {
    paged_reads(Cmd::DeviceInfo, DEVICE_INFO_SIZE)
}

pub fn config_reads() -> Vec<Frame> {
    paged_reads(Cmd::ConfigRead, CONFIG_TABLE_SIZE)
}

pub fn key_table_reads() -> Vec<Frame> {
    paged_reads(Cmd::KeyTableRead, KEY_TABLE_SIZE)
}

pub fn custom_led_reads() -> Vec<Frame> {
    paged_reads(Cmd::CustomLedRead, LED_TABLE_SIZE)
}

pub fn actuation_reads() -> Vec<Frame> {
    paged_reads(Cmd::ActuationRead, ACT_TABLE_SIZE)
}

pub fn light_read() -> Frame {
    header(Cmd::LightRead, LIGHT_BLOCK_SIZE, 0, true)
}

// -------------------------------------------------------------- replies ----

/// The reply frame without any report-id prefix some hidapi builds prepend.
pub fn normalize_reply(raw: &[u8]) -> Option<&[u8]> {
    match raw {
        [REPLY_MAGIC, ..] => Some(raw),
        [0x00, REPLY_MAGIC, ..] => Some(&raw[1..]),
        _ => None,
    }
}

/// The reply to `req` if `raw` is one: 0x55 magic, echoed command and offset.
pub fn match_reply<'a>(raw: &'a [u8], req: &Frame) -> Option<&'a [u8]> {
    let r = normalize_reply(raw)?;
    (r.len() >= PAYLOAD_START && r[1] == req[1] && r[3] == req[3] && r[4] == req[4]).then_some(r)
}

/// Copy one paged reply's payload into its place in `table`.
pub fn assemble(table: &mut [u8], reply: &[u8]) {
    let len = (reply[2] as usize).min(PAGE);
    let off = reply[3] as usize | (reply[4] as usize) << 8;
    for j in 0..len {
        if off + j < table.len() && PAYLOAD_START + j < reply.len() {
            table[off + j] = reply[PAYLOAD_START + j];
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    /// BCD-ish firmware version, e.g. 0x0153 = V1.53.
    pub firmware: u16,
}

impl DeviceInfo {
    /// Parse the offset-0 0x10 reply (all fields little-endian).
    pub fn parse(reply: &[u8]) -> Option<DeviceInfo> {
        let r = normalize_reply(reply)?;
        if r.len() < 18 || r[1] != Cmd::DeviceInfo.byte() {
            return None;
        }
        let le = |i: usize| r[i] as u16 | (r[i + 1] as u16) << 8;
        Some(DeviceInfo { vid: le(12), pid: le(14), firmware: le(16) })
    }
}

// ------------------------------------------------------------- lighting ----

/// The 0x13/0x23 lighting block.
///
/// On the F75 HE the 0x13 read-back reports the mode, brightness and speed of
/// the active effect, but its colour bytes keep the last stored colour even
/// after a 0x23 write that visibly changed it. Restore from a saved block
/// rather than trusting a fresh colour read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lighting {
    pub mode: u8,
    pub rgb: [u8; 3],
    pub alpha: u8,
    /// 1 = firmware picks the colours (multicolour), 0 = use `rgb`.
    pub color_mode: u8,
    pub brightness: u8,
    pub speed: u8,
    /// Bytes 13..16, carried through untouched for byte-exact restores.
    pub reserved: [u8; 3],
}

impl Lighting {
    pub fn from_reply(reply: &[u8]) -> Option<Lighting> {
        let r = normalize_reply(reply)?;
        if r.len() < 19 || r[1] != Cmd::LightRead.byte() {
            return None;
        }
        Some(Lighting {
            mode: r[8],
            rgb: [r[9], r[10], r[11]],
            alpha: r[12],
            color_mode: r[16],
            brightness: r[17],
            speed: r[18],
            reserved: [r[13], r[14], r[15]],
        })
    }

    pub fn write_frame(&self) -> Frame {
        let mut f = header(Cmd::LightWrite, LIGHT_BLOCK_SIZE, 0, true);
        f[8] = self.mode;
        f[9..12].copy_from_slice(&self.rgb);
        f[12] = self.alpha;
        f[13..16].copy_from_slice(&self.reserved);
        f[16] = self.color_mode;
        f[17] = self.brightness;
        f[18] = self.speed;
        f[22] = 0xAA;
        f[23] = 0x55;
        f
    }
}

// ------------------------------------------------------------ actuation ----

pub const TRIGGER_UNIT_MM: f64 = 0.01;

/// mm -> firmware units (0.01 mm), rounding exactly like the Python reference.
pub fn mm_to_raw(mm: f64) -> u16 {
    (mm / TRIGGER_UNIT_MM).round_ties_even().clamp(0.0, u16::MAX as f64) as u16
}

pub fn raw_to_mm(raw: u16) -> f64 {
    raw as f64 / 100.0
}

/// One 8-byte per-key record: `[axis][flags][trip][press][release]`, LE16 in 0.01 mm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActRecord {
    /// Switch model id. Kept as read; never invented.
    pub axis: u8,
    /// bit0 = rapid trigger.
    pub flags: u8,
    pub trip: u16,
    pub press: u16,
    pub release: u16,
}

impl ActRecord {
    pub fn from_bytes(b: &[u8]) -> ActRecord {
        let le = |i: usize| b[i] as u16 | (b[i + 1] as u16) << 8;
        ActRecord { axis: b[0], flags: b[1], trip: le(2), press: le(4), release: le(6) }
    }

    pub fn to_bytes(&self) -> [u8; ACT_RECORD_SIZE] {
        let [t0, t1] = self.trip.to_le_bytes();
        let [p0, p1] = self.press.to_le_bytes();
        let [r0, r1] = self.release.to_le_bytes();
        [self.axis, self.flags, t0, t1, p0, p1, r0, r1]
    }

    pub fn is_empty(&self) -> bool {
        *self == ActRecord::default()
    }

    pub fn rapid_trigger(&self) -> bool {
        self.flags & 0x01 != 0
    }
}

/// What to write to the selected keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Actuation {
    /// Fixed actuation point; any stored RT sensitivities are kept but disabled.
    Fixed { travel_mm: f64 },
    /// Rapid trigger. A zero release falls back to the press value.
    RapidTrigger { travel_mm: f64, press_mm: f64, release_mm: f64 },
    /// Clear the key's record: the firmware default, as the board ships
    /// (an all-zero table is the factory state read from the F75).
    Default,
}

/// The whole 1008-byte actuation table, kept as raw bytes so untouched keys
/// are written back exactly as read.
#[derive(Clone, PartialEq, Eq)]
pub struct ActTable(pub [u8; ACT_TABLE_SIZE]);

impl Default for ActTable {
    fn default() -> Self {
        ActTable([0; ACT_TABLE_SIZE])
    }
}

impl fmt::Debug for ActTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let used = (0..KEY_SLOTS).filter(|&i| !self.record(i).is_empty()).count();
        write!(f, "ActTable({used} configured slots)")
    }
}

impl ActTable {
    pub fn from_slice(b: &[u8]) -> Result<ActTable, ProtoError> {
        let arr = b
            .try_into()
            .map_err(|_| ProtoError::BadLength { expected: ACT_TABLE_SIZE, got: b.len() })?;
        Ok(ActTable(arr))
    }

    pub fn record(&self, idx: usize) -> ActRecord {
        let o = idx * ACT_RECORD_SIZE;
        ActRecord::from_bytes(&self.0[o..o + ACT_RECORD_SIZE])
    }

    pub fn set_record(&mut self, idx: usize, rec: ActRecord) {
        let o = idx * ACT_RECORD_SIZE;
        self.0[o..o + ACT_RECORD_SIZE].copy_from_slice(&rec.to_bytes());
    }

    /// Patch only `indices`, preserving each key's axis id and every other key.
    /// Same semantics as `Mini60Driver.set_actuation`.
    pub fn apply(&mut self, indices: &[usize], setting: Actuation) {
        for &i in indices.iter().filter(|&&i| i < KEY_SLOTS) {
            let old = self.record(i);
            let new = match setting {
                Actuation::Fixed { travel_mm } => ActRecord {
                    flags: old.flags & !0x01,
                    trip: mm_to_raw(travel_mm),
                    ..old
                },
                Actuation::RapidTrigger { travel_mm, press_mm, release_mm } => {
                    let release = if release_mm <= 0.0 { press_mm } else { release_mm };
                    ActRecord {
                        axis: old.axis,
                        flags: old.flags | 0x01,
                        trip: mm_to_raw(travel_mm),
                        press: mm_to_raw(press_mm),
                        release: mm_to_raw(release),
                    }
                }
                Actuation::Default => ActRecord::default(),
            };
            self.set_record(i, new);
        }
    }

    /// The full 0x27 write pass (18 frames, last-chunk flag on the final one).
    pub fn write_frames(&self) -> Vec<Frame> {
        paged_writes(Cmd::ActuationWrite, &self.0)
    }
}

// ------------------------------------------------------- per-key colour ----

/// `[index, r, g, b]` per key, the record format shared by 0x14/0x24/0x32.
pub fn led_table(colors: &[[u8; 3]; KEY_SLOTS]) -> [u8; LED_TABLE_SIZE] {
    let mut t = [0u8; LED_TABLE_SIZE];
    for (i, c) in colors.iter().enumerate() {
        let o = i * LED_RECORD_SIZE;
        t[o] = i as u8;
        t[o + 1..o + 4].copy_from_slice(c);
    }
    t
}

/// One live host-effect frame (9 x 0x32). Shown only while the board is in
/// custom mode (lighting mode 20); nothing is stored in flash.
pub fn stream_frames(table: &[u8; LED_TABLE_SIZE]) -> Vec<Frame> {
    paged_writes(Cmd::StreamLed, table)
}

/// 0x36: stop the live stream and hand the LEDs back to the firmware.
pub fn stream_clear() -> Frame {
    header(Cmd::ClearLed, CLEAR_LEN, 0, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dangerous_commands_are_unrepresentable_and_refused() {
        for c in DANGEROUS_CMDS {
            assert_eq!(Cmd::from_byte(c), None);
            let mut f = [0u8; FRAME_LEN];
            f[0] = MAGIC;
            f[1] = c;
            assert_eq!(guard(&f), Err(ProtoError::Refused(c)));
        }
    }

    #[test]
    fn guard_rejects_unknown_and_malformed() {
        let mut f = [0u8; FRAME_LEN];
        f[0] = MAGIC;
        f[1] = 0x22; // key-table write: real, but not enabled for this board yet
        assert_eq!(guard(&f), Err(ProtoError::Unknown(0x22)));
        f[0] = 0x00;
        assert_eq!(guard(&f), Err(ProtoError::BadMagic(0)));
        assert!(matches!(guard(&f[..10]), Err(ProtoError::BadLength { .. })));
    }

    #[test]
    fn read_commands_are_classified() {
        for c in Cmd::ALL {
            assert_eq!(c.is_read(), c.byte() < 0x20, "{c:?}");
        }
    }

    #[test]
    fn apply_preserves_axis_and_neighbours() {
        let mut t = ActTable::default();
        t.set_record(3, ActRecord { axis: 7, flags: 2, trip: 100, press: 20, release: 30 });
        let before_neighbour = t.record(4);
        t.apply(&[3], Actuation::Fixed { travel_mm: 1.5 });
        assert_eq!(t.record(3), ActRecord { axis: 7, flags: 2, trip: 150, press: 20, release: 30 });
        t.apply(&[3], Actuation::RapidTrigger { travel_mm: 1.5, press_mm: 0.3, release_mm: 0.0 });
        assert_eq!(t.record(3), ActRecord { axis: 7, flags: 3, trip: 150, press: 30, release: 30 });
        assert_eq!(t.record(4), before_neighbour);
        t.apply(&[3], Actuation::Default);
        assert!(t.record(3).is_empty());
        assert_eq!(t, ActTable::default(), "back to the factory table");
    }

    #[test]
    fn mm_rounding_matches_python() {
        assert_eq!(mm_to_raw(1.5), 150);
        assert_eq!(mm_to_raw(0.31), 31);
        assert_eq!(mm_to_raw(3.4), 340);
        assert_eq!(mm_to_raw(-1.0), 0);
    }
}
