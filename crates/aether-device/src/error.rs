use aether_proto::ProtoError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no supported keyboard is connected")]
    NotFound,
    #[error("HID: {0}")]
    Hid(String),
    #[error("protocol: {0}")]
    Proto(#[from] ProtoError),
    #[error("no reply to command 0x{cmd:02X} at offset 0x{offset:04X}")]
    Timeout { cmd: u8, offset: u16 },
    #[error("unexpected reply: {0}")]
    BadReply(String),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("invalid value: {0}")]
    Invalid(String),
    #[error("data files: {0}")]
    Data(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{} is connected over Bluetooth{}: typing only. Settings need the USB-C cable or the 2.4 GHz receiver", .0.board.name, .0.battery.map(|b| format!(" (battery {b}%)")).unwrap_or_default())]
    BluetoothOnly(Box<crate::BtKeyboard>),
}

impl Error {
    pub fn hid(e: impl std::fmt::Display) -> Error {
        Error::Hid(e.to_string())
    }
}
