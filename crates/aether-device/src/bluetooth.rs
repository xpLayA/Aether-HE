//! Recognising a board that is connected over Bluetooth only. Its BLE report
//! map has no vendor collection (fixtures/f75/bt_report_descriptor.txt), so
//! there is nothing to drive: we only name it and read BlueZ's battery level.
//! The hidraw node is never opened.

use hidapi::{BusType, HidApi};

use crate::{Board, Registry};

#[derive(Debug, Clone)]
pub struct BtKeyboard {
    pub board: Board,
    /// The Bluetooth name, e.g. "AULA F75 HE-1" (the suffix is the BT channel).
    pub name: String,
    pub mac: String,
    pub battery: Option<u8>,
}

/// The first registered board connected over Bluetooth, if any.
pub fn find(api: &HidApi, reg: &Registry) -> Option<BtKeyboard> {
    for board in &reg.boards {
        let Some(bt) = &board.bluetooth else { continue };
        let found = api.device_list().find(|d| {
            matches!(d.bus_type(), BusType::Bluetooth)
                && (d.vendor_id(), d.product_id()) == (bt.vid, bt.pid)
                && d.product_string().is_some_and(|p| p.starts_with(&bt.name_prefix))
        });
        if let Some(d) = found {
            let mac = d.serial_number().unwrap_or_default().to_string();
            return Some(BtKeyboard {
                board: board.clone(),
                name: d.product_string().unwrap_or_default().to_string(),
                battery: battery(&mac),
                mac,
            });
        }
    }
    None
}

/// BlueZ object path of `mac` under adapter `hciN`.
fn device_path(adapter: u8, mac: &str) -> String {
    format!("/org/bluez/hci{adapter}/dev_{}", mac.to_ascii_uppercase().replace(':', "_"))
}

/// `org.bluez.Battery1.Percentage` (the GATT Battery Service), or None.
fn battery(mac: &str) -> Option<u8> {
    if mac.is_empty() {
        return None;
    }
    let bus = zbus::blocking::Connection::system().ok()?;
    (0..4).find_map(|n| {
        let p = zbus::blocking::Proxy::new(&bus, "org.bluez", device_path(n, mac), "org.bluez.Battery1").ok()?;
        p.get_property::<u8>("Percentage").ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluez_path_from_hid_uniq() {
        assert_eq!(device_path(0, "02:00:00:aa:bb:cc"), "/org/bluez/hci0/dev_02_00_00_AA_BB_CC");
    }
}
