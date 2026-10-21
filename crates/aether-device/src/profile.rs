//! Named, switchable setups: lighting + every key's actuation + an optional
//! host effect. Stored as `~/.config/aether-he/profiles/<name>.json`.
//! Applying one backs the board up first and verifies the actuation read-back.

use std::path::{Path, PathBuf};

use aether_proto::{ActTable, Actuation};
use serde::{Deserialize, Serialize};

use crate::backup::{SavedLighting, hex};
use crate::{Backup, Error, Keyboard, Result};

/// One-click actuation settings, used by the GUI buttons and starter profiles.
pub const PRESETS: [(&str, Actuation); 5] = [
    ("Competitive", Actuation::RapidTrigger { travel_mm: 0.40, press_mm: 0.10, release_mm: 0.10 }),
    ("Gaming", Actuation::RapidTrigger { travel_mm: 0.80, press_mm: 0.15, release_mm: 0.15 }),
    ("Balanced", Actuation::Fixed { travel_mm: 1.20 }),
    ("Typing", Actuation::Fixed { travel_mm: 2.00 }),
    ("Default", Actuation::Default),
];

/// A host effect to start with the profile (runs while the GUI is open).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectSpec {
    pub kind: String,
    pub palette: Vec<[u8; 3]>,
    pub bg: [u8; 3],
    pub speed: f32,
    pub brightness: f32,
    /// 0 right, 1 left, 2 up, 3 down
    pub direction: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub format: u32,
    pub name: String,
    pub board: String,
    pub lighting: SavedLighting,
    /// The whole actuation table, hex.
    pub actuation: String,
    #[serde(default)]
    pub effect: Option<EffectSpec>,
}

/// `~/.config/aether-he/profiles`
pub fn profiles_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "aether-he")
        .ok_or_else(|| Error::Data("no home directory".into()))?;
    Ok(dirs.config_dir().join("profiles"))
}

/// A safe file name for a profile name ("Gaming / FPS" -> "gaming-fps").
fn file_stem(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "profile".into() } else { s }
}

impl Profile {
    /// The board's current lighting and actuation under `name`.
    pub fn capture(kb: &mut dyn Keyboard, name: &str) -> Result<Profile> {
        let b = Backup::capture(kb)?;
        Ok(Profile { format: 1, name: name.trim().to_string(), board: b.board, lighting: b.lighting, actuation: b.actuation, effect: None })
    }

    /// A profile with `setting` on every key and `lighting`.
    pub fn from_preset(name: &str, kb: &dyn Keyboard, lighting: SavedLighting, setting: Actuation) -> Profile {
        let mut t = ActTable::default();
        let keys: Vec<usize> = kb.layout().keys.iter().map(|k| k.index).collect();
        t.apply(&keys, setting);
        Profile { format: 1, name: name.into(), board: kb.board().slug.clone(), lighting, actuation: hex(&t.0), effect: None }
    }

    pub fn actuation_table(&self) -> Result<ActTable> {
        self.as_backup().actuation_table()
    }

    fn as_backup(&self) -> Backup {
        Backup { format: 1, created_unix: 0, board: self.board.clone(), lighting: self.lighting, actuation: self.actuation.clone() }
    }

    /// Back up the board, then write this profile's lighting and actuation
    /// (verified). Starting the effect is up to the caller.
    pub fn apply(&self, kb: &mut dyn Keyboard) -> Result<PathBuf> {
        if self.board != kb.board().slug {
            return Err(Error::Invalid(format!("profile {:?} is for {}, connected board is {}", self.name, self.board, kb.board().slug)));
        }
        self.actuation_table()?;
        let backup = Backup::snapshot(kb)?;
        self.as_backup().restore(kb)?;
        Ok(backup)
    }

    pub fn path_in(dir: &Path, name: &str) -> PathBuf {
        dir.join(format!("{}.json", file_stem(name)))
    }

    pub fn save_in(&self, dir: &Path) -> Result<PathBuf> {
        if self.name.trim().is_empty() {
            return Err(Error::Invalid("profile name is empty".into()));
        }
        std::fs::create_dir_all(dir)?;
        let path = Profile::path_in(dir, &self.name);
        let text = serde_json::to_string_pretty(self).map_err(|e| Error::Data(e.to_string()))?;
        std::fs::write(&path, text + "\n")?;
        Ok(path)
    }

    pub fn load(path: &Path) -> Result<Profile> {
        let text = std::fs::read_to_string(path)?;
        let p: Profile = serde_json::from_str(&text).map_err(|e| Error::Data(format!("{}: {e}", path.display())))?;
        p.actuation_table()?;
        Ok(p)
    }

    /// All readable profiles in `dir`, by name. Unreadable files are skipped.
    pub fn list_in(dir: &Path) -> Vec<Profile> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut v: Vec<Profile> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| Profile::load(&p).map_err(|e| log::warn!("skipping profile: {e}")).ok())
            .collect();
        v.sort_by_key(|p| p.name.to_lowercase());
        v
    }

    pub fn find_in(dir: &Path, name: &str) -> Option<Profile> {
        Profile::list_in(dir).into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn delete_in(dir: &Path, name: &str) -> Result<()> {
        let p = Profile::path_in(dir, name);
        if !p.is_file() {
            return Err(Error::Invalid(format!("no profile named {name:?}")));
        }
        Ok(std::fs::remove_file(p)?)
    }

    pub fn rename_in(dir: &Path, old: &str, new: &str) -> Result<PathBuf> {
        let mut p = Profile::find_in(dir, old).ok_or_else(|| Error::Invalid(format!("no profile named {old:?}")))?;
        if file_stem(old) != file_stem(new) && Profile::path_in(dir, new).exists() {
            return Err(Error::Invalid(format!("a profile named {new:?} already exists")));
        }
        Profile::delete_in(dir, old)?;
        p.name = new.trim().to_string();
        p.save_in(dir)
    }

    /// First run: Default / Gaming / Typing, all with the board's current lighting.
    pub fn ensure_starters(kb: &mut dyn Keyboard, dir: &Path) -> Result<()> {
        if !Profile::list_in(dir).is_empty() {
            return Ok(());
        }
        let lighting: SavedLighting = kb.lighting()?.into();
        for (name, setting) in PRESETS.iter().filter(|(n, _)| matches!(*n, "Default" | "Gaming" | "Typing")) {
            Profile::from_preset(name, kb, lighting, *setting).save_in(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hfd::tests::{FakeBoard, f75};

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aether-prof-{tag}-{}", std::process::id()))
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_stem("Gaming / FPS"), "gaming-fps");
        assert_eq!(file_stem("../../etc"), "etc");
        assert_eq!(file_stem("   "), "profile");
    }

    #[test]
    fn save_apply_round_trip() {
        let dir = tmp("rt");
        let mut kb = f75(FakeBoard::new());
        kb.set_actuation(&[34], Actuation::Fixed { travel_mm: 2.5 }).unwrap();
        let mine = Profile::capture(&mut kb, "Mine").unwrap();
        mine.save_in(&dir).unwrap();

        kb.set_actuation(&[34, 49], Actuation::Fixed { travel_mm: 0.5 }).unwrap();
        let loaded = Profile::find_in(&dir, "mine").unwrap();
        assert_eq!(loaded, mine);
        loaded.apply(&mut kb).unwrap();
        let t = kb.actuation_table().unwrap();
        assert_eq!((t.record(34).trip, t.record(49).trip), (250, 0));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn starters_presets_rename_delete() {
        let dir = tmp("st");
        let mut kb = f75(FakeBoard::new());
        Profile::ensure_starters(&mut kb, &dir).unwrap();
        let names: Vec<String> = Profile::list_in(&dir).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Default", "Gaming", "Typing"]);

        let gaming = Profile::find_in(&dir, "Gaming").unwrap().actuation_table().unwrap();
        let w = gaming.record(34);
        assert_eq!((w.trip, w.press, w.release, w.rapid_trigger()), (80, 15, 15, true));
        assert!(gaming.record(13).is_empty(), "slots that aren't keys stay empty");
        assert_eq!(Profile::find_in(&dir, "Default").unwrap().actuation_table().unwrap(), ActTable::default());

        Profile::ensure_starters(&mut kb, &dir).unwrap(); // no duplicates on later runs
        assert_eq!(Profile::list_in(&dir).len(), 3);
        Profile::rename_in(&dir, "Typing", "Office").unwrap();
        assert!(Profile::find_in(&dir, "Office").is_some() && Profile::find_in(&dir, "Typing").is_none());
        assert!(Profile::rename_in(&dir, "Office", "gaming").is_err(), "no clobbering");
        Profile::delete_in(&dir, "Office").unwrap();
        assert_eq!(Profile::list_in(&dir).len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn wrong_board_is_refused() {
        let mut kb = f75(FakeBoard::new());
        let mut p = Profile::capture(&mut kb, "x").unwrap();
        p.board = "some-other-board".into();
        assert!(p.apply(&mut kb).is_err());
    }
}
