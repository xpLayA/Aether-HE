use aether_proto::{FRAME_LEN, Frame, PAYLOAD_START, guard, to_report};
use hidapi::{HidApi, HidDevice};

use crate::{Board, Error, Link, Result};

/// A frame pipe to one keyboard. Implementations must pass every frame
/// through [`aether_proto::guard`] before it reaches the device.
pub trait Transport: Send {
    fn write(&mut self, frame: &Frame) -> Result<()>;
    /// One input report, or an empty vec on timeout.
    fn read(&mut self, timeout_ms: i32) -> Result<Vec<u8>>;
    /// Report size of the link; frames are paged to fit it.
    fn frame_len(&self) -> usize {
        FRAME_LEN
    }
}

pub struct HidTransport {
    dev: HidDevice,
    frame_len: usize,
}

impl HidTransport {
    /// Open the board's vendor collection on `link`, matched by usage page
    /// (and product string, for shared receivers). Never falls back to another
    /// interface: writing to the keyboard collection would be wrong.
    pub fn open(api: &HidApi, board: &Board, link: &Link) -> Result<HidTransport> {
        let info = api.device_list().find(|d| link.matches(d) && d.usage_page() == link.usage_page).ok_or_else(|| {
            Error::Hid(format!("{} is attached but its 0x{:04X} interface was not found", board.name, link.usage_page))
        })?;
        let dev = info.open_device(api).map_err(|e| {
            Error::Hid(format!(
                "cannot open {} ({e}); is the udev rule installed (scripts/install.sh) and the vendor web driver closed?",
                info.path().to_string_lossy()
            ))
        })?;
        log::info!("opened {} at {} ({} B reports)", board.name, info.path().to_string_lossy(), link.frame_len);
        Ok(HidTransport { dev, frame_len: link.frame_len })
    }
}

impl Link {
    /// Whether `d` is (an interface of) this link's USB device.
    pub fn matches(&self, d: &hidapi::DeviceInfo) -> bool {
        d.vendor_id() == self.vid
            && d.product_id() == self.pid
            && self.product.as_deref().is_none_or(|p| d.product_string() == Some(p))
    }
}

impl Transport for HidTransport {
    fn write(&mut self, frame: &Frame) -> Result<()> {
        guard(frame)?;
        if PAYLOAD_START + frame[2] as usize > self.frame_len {
            return Err(Error::Invalid(format!("chunk of {} bytes does not fit a {}-byte report", frame[2], self.frame_len)));
        }
        let report = &to_report(frame)[..self.frame_len + 1];
        let n = self.dev.write(report).map_err(Error::hid)?;
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

    fn frame_len(&self) -> usize {
        self.frame_len
    }
}
