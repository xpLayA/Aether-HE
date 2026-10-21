use aether_proto::{FRAME_LEN, Frame, guard, to_report};
use hidapi::{HidApi, HidDevice};

use crate::{Board, Error, Result};

/// A frame pipe to one keyboard. Implementations must pass every frame
/// through [`aether_proto::guard`] before it reaches the device.
pub trait Transport: Send {
    fn write(&mut self, frame: &Frame) -> Result<()>;
    /// One input report, or an empty vec on timeout.
    fn read(&mut self, timeout_ms: i32) -> Result<Vec<u8>>;
}

pub struct HidTransport {
    dev: HidDevice,
}

impl HidTransport {
    /// Open the board's vendor collection, matched by usage page. Never falls
    /// back to another interface: writing to the keyboard collection would be wrong.
    pub fn open(api: &HidApi, board: &Board) -> Result<HidTransport> {
        let info = api
            .device_list()
            .find(|d| {
                d.vendor_id() == board.vid && d.product_id() == board.pid && d.usage_page() == board.usage_page
            })
            .ok_or_else(|| {
                Error::Hid(format!(
                    "{} is attached but its 0x{:04X} interface was not found",
                    board.name, board.usage_page
                ))
            })?;
        let dev = info.open_device(api).map_err(|e| {
            Error::Hid(format!(
                "cannot open {} ({e}); is the udev rule installed (scripts/install.sh) and the vendor web driver closed?",
                info.path().to_string_lossy()
            ))
        })?;
        log::info!("opened {} at {}", board.name, info.path().to_string_lossy());
        Ok(HidTransport { dev })
    }
}

impl Transport for HidTransport {
    fn write(&mut self, frame: &Frame) -> Result<()> {
        guard(frame)?;
        let report = to_report(frame);
        let n = self.dev.write(&report).map_err(Error::hid)?;
        if n != report.len() {
            return Err(Error::Hid(format!("short write: {n}/{} bytes", report.len())));
        }
        Ok(())
    }

    fn read(&mut self, timeout_ms: i32) -> Result<Vec<u8>> {
        let mut buf = [0u8; FRAME_LEN + 1];
        let n = self.dev.read_timeout(&mut buf, timeout_ms).map_err(Error::hid)?;
        Ok(buf[..n].to_vec())
    }
}
