//! Key presses from the board's own keyboard collections, for reactive
//! lighting. Read-only: the devices opened here are never written to, and
//! hidraw reads are shared, so typing reaches the desktop as usual.
//!
//! Only "key N went down" leaves this module (as a layout index); nothing is
//! logged or stored, and it runs only while a reactive effect is on.
//!
//! The F75 reports keys two ways (report descriptors, fixtures/f75/README.md):
//! interface 0 is a boot keyboard, `[mods][0][6 usages]`, and interface 1
//! report ID 1 is an NKRO bitmap of usages 0x00..0x77. Both are read, and a
//! press is a usage that wasn't down in the previous report from that source.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use crate::{Board, Error, Layout, Result};

/// HID usage (keyboard page 0x07) for a layout `code` (DOM `KeyboardEvent.code` names).
pub fn usage_for_code(code: &str) -> Option<u8> {
    if let Some(c) = code.strip_prefix("Key").and_then(|s| s.chars().next().filter(|c| c.is_ascii_uppercase() && code.len() == 4)) {
        return Some(0x04 + (c as u8 - b'A'));
    }
    if let Some(d) = code.strip_prefix("Digit").and_then(|s| s.parse::<u8>().ok()) {
        return Some(if d == 0 { 0x27 } else { 0x1D + d });
    }
    if let Some(n) = code.strip_prefix('F').and_then(|s| s.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)) {
        return Some(0x39 + n);
    }
    Some(match code {
        "Enter" => 0x28,
        "Escape" => 0x29,
        "Backspace" => 0x2A,
        "Tab" => 0x2B,
        "Space" => 0x2C,
        "Minus" => 0x2D,
        "Equal" => 0x2E,
        "BracketLeft" => 0x2F,
        "BracketRight" => 0x30,
        "Backslash" => 0x31,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "CapsLock" => 0x39,
        "Insert" => 0x49,
        "Home" => 0x4A,
        "PageUp" => 0x4B,
        "Delete" => 0x4C,
        "End" => 0x4D,
        "PageDown" => 0x4E,
        "ArrowRight" => 0x4F,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "ControlLeft" => 0xE0,
        "ShiftLeft" => 0xE1,
        "AltLeft" => 0xE2,
        "MetaLeft" => 0xE3,
        "ControlRight" => 0xE4,
        "ShiftRight" => 0xE5,
        "AltRight" => 0xE6,
        "MetaRight" => 0xE7,
        _ => return None,
    })
}

/// The usages held down in one input report, or `None` for reports that
/// aren't key state (consumer, system, mouse).
pub fn parse_report(r: &[u8]) -> Option<BTreeSet<u8>> {
    let mods = |m: u8| (0..8).filter(move |b| m & (1 << b) != 0).map(|b| 0xE0 + b);
    match r {
        // NKRO: report ID 1, one bit per usage from 0x00
        [1, bits @ ..] if bits.len() >= 15 => {
            Some((0..bits.len() * 8).filter(|&i| bits[i / 8] & (1 << (i % 8)) != 0).map(|i| i as u8).collect())
        }
        // boot keyboard (no report ID); usages 1..3 are rollover/error codes
        [m, _, keys @ ..] if r.len() == 8 => Some(mods(*m).chain(keys.iter().copied().filter(|&k| k > 3)).collect()),
        _ => None,
    }
}

/// Presses on the connected board, as layout key indices.
pub struct KeyPresses {
    rx: Receiver<usize>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl KeyPresses {
    pub fn open(board: &Board, layout: &Layout) -> Result<KeyPresses> {
        let api = hidapi::HidApi::new().map_err(Error::hid)?;
        let by_usage: HashMap<u8, usize> =
            layout.keys.iter().filter_map(|k| usage_for_code(&k.code).map(|u| (u, k.index))).collect();
        let mut paths: Vec<_> = api
            .device_list()
            .filter(|d| d.vendor_id() == board.vid && d.product_id() == board.pid && d.usage_page() == 0x01 && d.usage() == 0x06)
            .map(|d| d.path().to_owned())
            .collect();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return Err(Error::Hid(format!("{} has no keyboard interface to read key presses from", board.name)));
        }
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        for path in paths {
            let dev = api.open_path(&path).map_err(|e| Error::Hid(format!("cannot read key presses: {e}")))?;
            let (tx, stop, map) = (tx.clone(), stop.clone(), by_usage.clone());
            let t = std::thread::Builder::new()
                .name("aether-keys".into())
                .spawn(move || listen(dev, map, tx, stop))
                .map_err(|e| Error::Hid(e.to_string()))?;
            threads.push(t);
        }
        Ok(KeyPresses { rx, stop, threads })
    }

    /// Presses since the last call.
    pub fn drain(&self) -> impl Iterator<Item = usize> + '_ {
        self.rx.try_iter()
    }
}

impl Drop for KeyPresses {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn listen(dev: hidapi::HidDevice, map: HashMap<u8, usize>, tx: Sender<usize>, stop: Arc<AtomicBool>) {
    let mut down = BTreeSet::new();
    let mut buf = [0u8; 64];
    while !stop.load(Ordering::SeqCst) {
        let n = match dev.read_timeout(&mut buf, 100) {
            Ok(n) => n,
            Err(_) => return, // unplugged; the effect keeps running without reacting
        };
        let Some(now) = parse_report(&buf[..n]) else { continue };
        for u in now.difference(&down) {
            if let Some(&i) = map.get(u) {
                if tx.send(i).is_err() {
                    return;
                }
            }
        }
        down = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_f75_key_but_fn_has_a_usage() {
        let reg = crate::Registry::load().unwrap();
        let board = reg.boards.iter().find(|b| b.slug == "aula-f75-he").unwrap();
        let layout = Layout::load(board).unwrap();
        let missing: Vec<&str> = layout.keys.iter().filter(|k| usage_for_code(&k.code).is_none()).map(|k| k.code.as_str()).collect();
        assert_eq!(missing, ["KeyFn"], "Fn never reaches the host");
        let mut usages: Vec<u8> = layout.keys.iter().filter_map(|k| usage_for_code(&k.code)).collect();
        let n = usages.len();
        usages.sort();
        usages.dedup();
        assert_eq!(usages.len(), n, "no two keys share a usage");
        assert_eq!((usage_for_code("KeyA"), usage_for_code("Digit0"), usage_for_code("F12")), (Some(0x04), Some(0x27), Some(0x45)));
    }

    #[test]
    fn boot_and_nkro_reports() {
        // boot: Left Shift + A + D, plus a rollover error code that's ignored
        let boot = [0x02, 0, 0x04, 0x07, 0x01, 0, 0, 0];
        assert_eq!(parse_report(&boot), Some(BTreeSet::from([0x04, 0x07, 0xE1])));
        // NKRO: A (0x04) and Space (0x2C)
        let mut nkro = [0u8; 16];
        nkro[0] = 1;
        nkro[1 + 0x04 / 8] |= 1 << (0x04 % 8);
        nkro[1 + 0x2C / 8] |= 1 << (0x2C % 8);
        assert_eq!(parse_report(&nkro), Some(BTreeSet::from([0x04, 0x2C])));
        // consumer report (ID 3) is not key state
        assert_eq!(parse_report(&[3, 0xE9, 0]), None);
    }
}
