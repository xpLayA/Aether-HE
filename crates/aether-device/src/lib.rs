//! Talking to a real keyboard: board registry + layouts (`data/`), the hidraw
//! transport, per-protocol drivers behind the [`Keyboard`] trait, and
//! backup/restore of the board's settings.

mod advanced;
mod backup;
mod bluetooth;
mod data;
mod error;
mod hfd;
mod keys_in;
mod profile;
mod transport;

pub use advanced::{Advanced, describe, plan_advanced, usage_name};
pub use bluetooth::BtKeyboard;
pub use backup::{Backup, Colors, KeyTables, SavedLighting, backups_dir};
pub use data::{
    ActuationLimits, Board, BtId, Capabilities, Key, Layout, LightMode, LightingInfo, Link, Registry, data_dir,
};
pub use error::{Error, Result};
pub use hfd::Hfd;
pub use keys_in::{KeyPresses, usage_for_code};
pub use profile::{BUILTINS, EffectSpec, PRESETS, Profile, is_builtin_name, profiles_dir};
pub use transport::{HidTransport, Transport};

use aether_proto::{ActTable, Actuation, DeviceInfo, DksTable, KEY_SLOTS, KeyTable, Lighting};

/// Everything the apps can do to a board. One implementation per protocol
/// family; boards without one are never opened.
pub trait Keyboard: Send {
    fn board(&self) -> &Board;
    fn layout(&self) -> &Layout;
    fn device_info(&mut self) -> Result<DeviceInfo>;

    fn lighting(&mut self) -> Result<Lighting>;
    fn set_lighting(&mut self, l: &Lighting) -> Result<()>;

    fn actuation_table(&mut self) -> Result<ActTable>;
    /// Write a full table as-is (restore path).
    fn write_actuation_table(&mut self, t: &ActTable) -> Result<()>;
    /// Check `setting` against this board without touching it, so callers can
    /// refuse before taking a backup.
    fn validate_actuation(&self, indices: &[usize], setting: Actuation) -> Result<()>;
    /// Read-modify-write `setting` onto the physical keys in `indices`.
    fn set_actuation(&mut self, indices: &[usize], setting: Actuation) -> Result<()>;

    /// The per-key colours stored on the board (shown in lighting mode 20).
    fn custom_leds(&mut self) -> Result<[[u8; 3]; KEY_SLOTS]>;
    /// Store per-key colours, verified by read-back. Doesn't change the mode.
    fn set_custom_leds(&mut self, colors: &[[u8; 3]; KEY_SLOTS]) -> Result<()>;

    /// Every key's record (remaps and advanced keys).
    fn key_table(&mut self) -> Result<KeyTable>;
    /// Write the whole key table, verified by read-back.
    fn write_key_table(&mut self, t: &KeyTable) -> Result<()>;
    fn dks_table(&mut self) -> Result<DksTable>;
    /// Write the whole DKS table, verified by read-back.
    fn write_dks_table(&mut self, t: &DksTable) -> Result<()>;

    /// Switch to custom mode for host-streamed effects.
    fn stream_begin(&mut self) -> Result<()>;
    fn stream_frame(&mut self, colors: &[[u8; 3]; KEY_SLOTS]) -> Result<()>;
    /// Stop streaming; the caller restores the lighting it saved.
    fn stream_end(&mut self) -> Result<()>;
}

/// Open the first attached board and confirm it answers with its own identity.
/// The front ends use this rather than [`open_first`].
pub fn connect() -> Result<(Box<dyn Keyboard>, DeviceInfo)> {
    let reg = Registry::load()?;
    let mut kb = match open_first(&reg) {
        Err(Error::NotFound) => {
            let api = hidapi::HidApi::new().map_err(Error::hid)?;
            return Err(bluetooth::find(&api, &reg).map_or(Error::NotFound, |bt| Error::BluetoothOnly(Box::new(bt))));
        }
        r => r?,
    };
    let info = match kb.device_info() {
        // The receiver stays enumerated while the keyboard sleeps or is out of range.
        Err(Error::Timeout { .. }) if kb.board().wireless => {
            return Err(Error::Hid(format!("{} is not answering through its 2.4 GHz receiver (asleep or off?)", kb.board().name)));
        }
        r => r?,
    };
    Ok((kb, info))
}

/// Find the first registered, attached board and open it, preferring a
/// wired link over the dongle when both are present.
pub fn open_first(reg: &Registry) -> Result<Box<dyn Keyboard>> {
    let api = hidapi::HidApi::new().map_err(Error::hid)?;
    for board in &reg.boards {
        for link in board.all_links() {
            if api.device_list().any(|d| link.matches(d)) {
                return open(&api, board.through(&link), &link);
            }
        }
    }
    Err(Error::NotFound)
}

fn open(api: &hidapi::HidApi, board: Board, link: &Link) -> Result<Box<dyn Keyboard>> {
    match board.protocol.as_str() {
        "hfd" => {
            let layout = Layout::load(&board)?;
            let t = HidTransport::open(api, &board, link)?;
            Ok(Box::new(Hfd::new(t, board, layout)))
        }
        other => Err(Error::Unsupported(format!("protocol {other:?} has no driver"))),
    }
}
