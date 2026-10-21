//! Talking to a real keyboard: board registry + layouts (`data/`), the hidraw
//! transport, per-protocol drivers behind the [`Keyboard`] trait, and
//! backup/restore of the board's settings.

mod backup;
mod data;
mod error;
mod hfd;
mod profile;
mod transport;

pub use backup::{Backup, SavedLighting, backups_dir};
pub use data::{
    ActuationLimits, Board, Capabilities, Key, Layout, LightMode, LightingInfo, Registry, data_dir,
};
pub use error::{Error, Result};
pub use hfd::Hfd;
pub use profile::{BUILTINS, EffectSpec, PRESETS, Profile, is_builtin_name, profiles_dir};
pub use transport::{HidTransport, Transport};

use aether_proto::{ActTable, Actuation, DeviceInfo, KEY_SLOTS, Lighting};

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

    /// Switch to custom mode for host-streamed effects.
    fn stream_begin(&mut self) -> Result<()>;
    fn stream_frame(&mut self, colors: &[[u8; 3]; KEY_SLOTS]) -> Result<()>;
    /// Stop streaming; the caller restores the lighting it saved.
    fn stream_end(&mut self) -> Result<()>;
}

/// Open the first attached board and confirm it answers with its own identity.
/// The front ends use this rather than [`open_first`].
pub fn connect() -> Result<(Box<dyn Keyboard>, DeviceInfo)> {
    let mut kb = open_first(&Registry::load()?)?;
    let info = kb.device_info()?;
    Ok((kb, info))
}

/// Find the first registered, attached board and open it.
pub fn open_first(reg: &Registry) -> Result<Box<dyn Keyboard>> {
    let api = hidapi::HidApi::new().map_err(Error::hid)?;
    for board in &reg.boards {
        if api.device_list().any(|d| d.vendor_id() == board.vid && d.product_id() == board.pid) {
            return open(&api, board.clone());
        }
    }
    Err(Error::NotFound)
}

fn open(api: &hidapi::HidApi, board: Board) -> Result<Box<dyn Keyboard>> {
    match board.protocol.as_str() {
        "hfd" => {
            let layout = Layout::load(&board)?;
            let t = HidTransport::open(api, &board)?;
            Ok(Box::new(Hfd::new(t, board, layout)))
        }
        other => Err(Error::Unsupported(format!("protocol {other:?} has no driver"))),
    }
}
