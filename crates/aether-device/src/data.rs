//! `data/boards.json` (which boards exist and what is verified on each) and
//! the per-board layout files.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use crate::{Error, Result};

/// Where `boards.json` and `layouts/` live, first match wins:
/// `$AETHER_DATA_DIR`, `<exe>/../share/aether-he/data` (installed),
/// then the source tree's `data/` (cargo run).
pub fn data_dir() -> Result<PathBuf> {
    let mut tried = Vec::new();
    if let Some(d) = std::env::var_os("AETHER_DATA_DIR") {
        tried.push(PathBuf::from(d));
    }
    if let Some(bin) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        tried.push(bin.join("../share/aether-he/data"));
    }
    tried.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"));
    tried
        .iter()
        .find(|d| d.join("boards.json").is_file())
        .cloned()
        .ok_or_else(|| Error::Data(format!("boards.json not found in {tried:?}")))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Registry {
    pub boards: Vec<Board>,
}

impl Registry {
    pub fn load() -> Result<Registry> {
        let path = data_dir()?.join("boards.json");
        let text = std::fs::read_to_string(&path)?;
        serde_json::from_str(&text).map_err(|e| Error::Data(format!("{}: {e}", path.display())))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Board {
    pub slug: String,
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub protocol: String,
    pub layout: String,
    pub capabilities: Capabilities,
    pub lighting: LightingInfo,
    pub actuation: ActuationLimits,
    /// Other ways the board can be attached (the 2.4 GHz dongle); the wired
    /// `vid`/`pid`/`usage_page` above is always tried first.
    #[serde(default)]
    pub links: Vec<Link>,
    /// What is verified over a wireless link; replaces `capabilities` when
    /// the board is opened through one.
    #[serde(default)]
    pub wireless_capabilities: Capabilities,
    /// How the board names itself over Bluetooth (typing only there).
    #[serde(default)]
    pub bluetooth: Option<BtId>,
    /// Set when this board was opened through a wireless link.
    #[serde(skip)]
    pub wireless: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BtId {
    pub vid: u16,
    pub pid: u16,
    /// Start of the BLE name; the firmware appends the channel ("AULA F75 HE-1").
    pub name_prefix: String,
}

/// One USB identity a board can show up as.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Link {
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    /// Report size of the vendor collection (64 wired, 32 on the dongle).
    #[serde(default = "frame_len")]
    pub frame_len: usize,
    #[serde(default)]
    pub wireless: bool,
    /// USB product string, for receivers whose PID many boards share.
    #[serde(default)]
    pub product: Option<String>,
}

fn frame_len() -> usize {
    aether_proto::FRAME_LEN
}

impl Board {
    /// Every link, wired first.
    pub fn all_links(&self) -> Vec<Link> {
        let wired = Link { vid: self.vid, pid: self.pid, usage_page: self.usage_page, frame_len: frame_len(), wireless: false, product: None };
        std::iter::once(wired).chain(self.links.iter().cloned()).collect()
    }

    /// This board as seen through `link`: wireless links carry their own capabilities.
    pub fn through(&self, link: &Link) -> Board {
        let mut b = self.clone();
        if link.wireless {
            b.capabilities = self.wireless_capabilities.clone();
            b.wireless = true;
        }
        b
    }
}

/// Only what has been round-tripped on the board is true; defaults are off.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub lighting: bool,
    pub host_effects: bool,
    pub actuation: bool,
    pub rapid_trigger: bool,
    /// Per-key colours stored on the board (0x14/0x24, lighting mode 20).
    pub per_key_store: bool,
    /// Key records (0x12/0x22) and the DKS table (0x18/0x28): DKS, MT, TGL, SOCD, RS.
    pub advanced_keys: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightingInfo {
    pub brightness_max: u8,
    pub speed_max: u8,
    /// Sustainable host-stream rate measured on the board.
    pub host_fps: f32,
    pub modes: Vec<LightMode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightMode {
    pub byte: u8,
    pub name: String,
    /// false: the firmware picks the colours itself.
    #[serde(default = "yes")]
    pub color: bool,
    #[serde(default = "yes")]
    pub speed: bool,
}

fn yes() -> bool {
    true
}

impl LightingInfo {
    pub fn mode(&self, byte: u8) -> Option<&LightMode> {
        self.modes.iter().find(|m| m.byte == byte)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActuationLimits {
    pub travel_mm: [f64; 2],
    pub rt_mm: [f64; 2],
}

/// A physical key: firmware index plus its drawing rectangle (vendor pixel grid).
#[derive(Debug, Clone, Deserialize)]
pub struct Key {
    #[serde(deserialize_with = "num")]
    pub index: usize,
    pub name: String,
    pub code: String,
    #[serde(deserialize_with = "num")]
    pub x: f32,
    #[serde(deserialize_with = "num")]
    pub y: f32,
    #[serde(deserialize_with = "num")]
    pub width: f32,
    #[serde(deserialize_with = "num")]
    pub height: f32,
}

/// The layout files store numbers as strings ("42", "68.57").
fn num<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        S(String),
        N(serde_json::Number),
    }
    let s = match Raw::deserialize(d)? {
        Raw::S(s) => s,
        Raw::N(n) => n.to_string(),
    };
    s.trim().parse().map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Layout {
    pub keys: Vec<Key>,
}

impl Layout {
    pub fn load(board: &Board) -> Result<Layout> {
        let path = data_dir()?.join(&board.layout);
        Layout::from_json(&std::fs::read_to_string(&path)?)
            .map_err(|e| Error::Data(format!("{}: {e}", path.display())))
    }

    pub fn from_json(text: &str) -> std::result::Result<Layout, String> {
        let l: Layout = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if let Some(k) = l.keys.iter().find(|k| k.index >= aether_proto::KEY_SLOTS) {
            return Err(format!("key {} has index {} outside the key table", k.name, k.index));
        }
        Ok(l)
    }

    pub fn key(&self, index: usize) -> Option<&Key> {
        self.keys.iter().find(|k| k.index == index)
    }

    /// Look a key up by name ("W", "Esc") or browser code ("KeyW"), case-insensitive.
    pub fn find(&self, name: &str) -> Option<&Key> {
        self.keys
            .iter()
            .find(|k| k.name.eq_ignore_ascii_case(name) || k.code.eq_ignore_ascii_case(name))
            .or_else(|| {
                let code = format!("Key{name}");
                self.keys.iter().find(|k| k.code.eq_ignore_ascii_case(&code))
            })
    }

    pub fn contains(&self, index: usize) -> bool {
        self.key(index).is_some()
    }
}

/// The board's built-in Fn shortcuts, from its manual
/// (`data/shortcuts/<slug>.json`). Firmware features: shown, never sent.
#[derive(Debug, Clone, Deserialize)]
pub struct Shortcuts {
    pub note: String,
    pub groups: Vec<ShortcutGroup>,
    /// What the F-row does in each system mode (Fn+Q/W/E).
    pub frow: Vec<FRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShortcutGroup {
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub rows: Vec<Shortcut>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Shortcut {
    pub keys: String,
    pub does: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FRow {
    pub system: String,
    pub switch: String,
    /// F1..F12 pressed alone, then with Fn.
    pub press: Vec<String>,
    #[serde(rename = "fn")]
    pub with_fn: Vec<String>,
}

impl Shortcuts {
    /// `None` when the board has no shortcuts file.
    pub fn load(slug: &str) -> Result<Option<Shortcuts>> {
        let path = data_dir()?.join("shortcuts").join(format!("{slug}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        serde_json::from_str(&text).map(Some).map_err(|e| Error::Data(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_registry_and_f75_layout_load() {
        let reg = Registry::load().unwrap();
        let f75 = reg.boards.iter().find(|b| b.slug == "aula-f75-he").unwrap();
        assert_eq!((f75.vid, f75.pid, f75.usage_page), (0x38A6, 0x2729, 0xFF68));
        let l = Layout::load(f75).unwrap();
        assert_eq!(l.keys.len(), 80);
        assert_eq!(l.find("w").unwrap().index, 34);
        assert_eq!(l.find("Esc").unwrap().index, 0);
        assert_eq!(l.find("KeyA").unwrap().index, 49);
        assert!(f75.lighting.mode(11).is_some());
        let dongle = &f75.all_links()[1];
        assert_eq!((dongle.vid, dongle.pid, dongle.usage_page, dongle.frame_len), (0x0C45, 0xFEFE, 0xFF60, 32));
        let rf = f75.through(dongle);
        assert!(rf.wireless && rf.capabilities.actuation);
        assert!(!rf.capabilities.host_effects, "0x32 does not cross the RF bridge");
        assert_eq!(f75.bluetooth.as_ref().unwrap().name_prefix, "AULA F75 HE-");
    }

    #[test]
    fn f75_shortcuts_load() {
        let s = Shortcuts::load("aula-f75-he").unwrap().unwrap();
        assert_eq!(s.frow.iter().map(|f| f.system.as_str()).collect::<Vec<_>>(), ["Windows", "Mac", "Android"]);
        assert!(s.frow.iter().all(|f| f.press.len() == 12 && f.with_fn.len() == 12));
        assert!(s.groups.iter().all(|g| !g.rows.is_empty()));
        assert!(Shortcuts::load("no-such-board").unwrap().is_none());
    }
}
