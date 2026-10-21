//! Named, switchable setups: lighting + every key's actuation + an optional
//! host effect. Custom profiles are stored as
//! `~/.config/aether-he/profiles/<name>.json`; the built-ins come from
//! [`PRESETS`] and only set actuation. Applying one backs the board up first
//! and verifies the actuation read-back.

use std::path::{Path, PathBuf};

use aether_proto::{ActTable, Actuation};
use serde::{Deserialize, Serialize};

use crate::backup::{Colors, SavedLighting, colors_from_hex, colors_hex, hex, unhex, write_verified};
use crate::{Backup, Error, Keyboard, Layout, Result};

/// One-click actuation settings, used by the GUI buttons and the built-in profiles.
pub const PRESETS: [(&str, Actuation); 5] = [
    ("Competitive", Actuation::RapidTrigger { travel_mm: 0.40, press_mm: 0.10, release_mm: 0.10 }),
    ("Gaming", Actuation::RapidTrigger { travel_mm: 0.80, press_mm: 0.15, release_mm: 0.15 }),
    ("Balanced", Actuation::Fixed { travel_mm: 1.20 }),
    ("Typing", Actuation::Fixed { travel_mm: 2.00 }),
    ("Default", Actuation::Default),
];

/// Built-in profile names, in display order, with the preset each puts on every key.
pub const BUILTINS: [(&str, usize); 5] =
    [("Firmware default", 4), ("Competitive", 0), ("Gaming", 1), ("Balanced", 2), ("Typing", 3)];

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
    /// 1: lighting always present. 2: lighting optional.
    pub format: u32,
    pub name: String,
    pub board: String,
    /// `None` leaves the keyboard's lighting (and any running effect) alone.
    pub lighting: Option<SavedLighting>,
    /// The whole actuation table, hex.
    pub actuation: String,
    #[serde(default)]
    pub effect: Option<EffectSpec>,
    /// Stored per-key colours (lighting mode 20), hex; written with the lighting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_leds: Option<String>,
    /// Generated from [`BUILTINS`], never saved, can't be edited.
    #[serde(skip)]
    pub builtin: bool,
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

/// Custom profiles can't take a built-in's name, so a name always means one profile.
pub fn is_builtin_name(name: &str) -> bool {
    BUILTINS.iter().any(|(b, _)| b.eq_ignore_ascii_case(name.trim()))
}

impl Profile {
    pub fn new(name: &str, board: &str, lighting: Option<SavedLighting>, table: &ActTable, effect: Option<EffectSpec>) -> Profile {
        Profile {
            format: 2,
            name: name.trim().to_string(),
            board: board.into(),
            lighting,
            actuation: hex(&table.0),
            effect,
            custom_leds: None,
            builtin: false,
        }
    }

    pub fn with_custom_leds(self, c: Option<&Colors>) -> Profile {
        Profile { custom_leds: c.map(colors_hex), ..self }
    }

    pub fn custom_led_colors(&self) -> Result<Option<Colors>> {
        self.custom_leds.as_deref().map(colors_from_hex).transpose()
    }

    /// The board's current lighting and actuation under `name`.
    pub fn capture(kb: &mut dyn Keyboard, name: &str) -> Result<Profile> {
        let b = Backup::capture(kb)?;
        Ok(Profile {
            lighting: Some(b.lighting),
            actuation: b.actuation,
            custom_leds: b.custom_leds,
            ..Profile::new(name, &b.board, None, &ActTable::default(), None)
        })
    }

    /// `setting` on every key of `layout`, lighting untouched.
    pub fn from_preset(name: &str, board: &str, layout: &Layout, setting: Actuation) -> Profile {
        let mut t = ActTable::default();
        let keys: Vec<usize> = layout.keys.iter().map(|k| k.index).collect();
        t.apply(&keys, setting);
        Profile::new(name, board, None, &t, None)
    }

    /// The built-in profiles for a board, in display order.
    pub fn builtins(board: &str, layout: &Layout) -> Vec<Profile> {
        BUILTINS
            .iter()
            .map(|(name, preset)| Profile { builtin: true, ..Profile::from_preset(name, board, layout, PRESETS[*preset].1) })
            .collect()
    }

    pub fn actuation_table(&self) -> Result<ActTable> {
        Ok(ActTable::from_slice(&unhex(&self.actuation)?)?)
    }

    /// Back up the board, then write this profile's lighting (if it has any)
    /// and actuation (verified). Starting the effect is up to the caller.
    pub fn apply(&self, kb: &mut dyn Keyboard) -> Result<PathBuf> {
        if self.board != kb.board().slug {
            return Err(Error::Invalid(format!("profile {:?} is for {}, connected board is {}", self.name, self.board, kb.board().slug)));
        }
        let table = self.actuation_table()?;
        let leds = self.custom_led_colors()?;
        let backup = Backup::snapshot(kb)?;
        if let Some(c) = leds.filter(|_| self.lighting.is_some() && kb.board().capabilities.per_key_store) {
            kb.set_custom_leds(&c)?;
        }
        if let Some(l) = self.lighting {
            kb.set_lighting(&l.into())?;
        }
        write_verified(kb, &table)?;
        Ok(backup)
    }

    pub fn path_in(dir: &Path, name: &str) -> PathBuf {
        dir.join(format!("{}.json", file_stem(name)))
    }

    /// The file actually holding profile `name`: usually [`Profile::path_in`],
    /// but files written by older versions may be named differently ("Default.json").
    fn file_of(dir: &Path, name: &str) -> Option<PathBuf> {
        let usual = Profile::path_in(dir, name);
        if Profile::load(&usual).is_ok_and(|p| p.name.eq_ignore_ascii_case(name.trim())) {
            return Some(usual);
        }
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .find(|p| Profile::load(p).is_ok_and(|q| q.name.eq_ignore_ascii_case(name.trim())))
    }

    /// Save, replacing the file of a profile with this name. Refuses built-in
    /// names, and refuses to replace a *different* profile whose name maps to
    /// the same file ("Gaming!" vs "Gaming").
    pub fn save_in(&self, dir: &Path) -> Result<PathBuf> {
        self.check_name()?;
        if let Some(path) = Profile::file_of(dir, &self.name) {
            return self.write(dir, &path);
        }
        let path = Profile::path_in(dir, &self.name);
        if let Ok(other) = Profile::load(&path) {
            if !other.name.eq_ignore_ascii_case(&self.name) {
                return Err(Error::Invalid(format!("{:?} would replace the profile {:?}; pick another name", self.name, other.name)));
            }
        }
        self.write(dir, &path)
    }

    /// Save as a new profile; refuses if one with this name (or file) exists.
    pub fn save_new_in(&self, dir: &Path) -> Result<PathBuf> {
        self.check_name()?;
        let path = Profile::path_in(dir, &self.name);
        if path.exists() {
            return Err(Error::Invalid(format!("a profile named {:?} already exists", self.name)));
        }
        self.write(dir, &path)
    }

    fn check_name(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::Invalid("profile name is empty".into()));
        }
        if is_builtin_name(&self.name) {
            return Err(Error::Invalid(format!("{:?} is a built-in profile; pick another name", self.name)));
        }
        Ok(())
    }

    fn write(&self, dir: &Path, path: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let p = Profile { format: 2, ..self.clone() };
        let text = serde_json::to_string_pretty(&p).map_err(|e| Error::Data(e.to_string()))?;
        std::fs::write(path, text + "\n")?;
        Ok(path.to_path_buf())
    }

    pub fn load(path: &Path) -> Result<Profile> {
        let text = std::fs::read_to_string(path)?;
        let p: Profile = serde_json::from_str(&text).map_err(|e| Error::Data(format!("{}: {e}", path.display())))?;
        p.actuation_table()?;
        p.custom_led_colors()?;
        Ok(p)
    }

    /// All readable custom profiles in `dir` for `board` ("" = any), by name.
    /// Unreadable files are skipped.
    pub fn list_in(dir: &Path, board: &str) -> Vec<Profile> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut v: Vec<Profile> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| Profile::load(&p).map_err(|e| log::warn!("skipping profile: {e}")).ok())
            .filter(|p| board.is_empty() || p.board == board)
            .collect();
        v.sort_by_key(|p| p.name.to_lowercase());
        v
    }

    pub fn find_in(dir: &Path, name: &str) -> Option<Profile> {
        Profile::list_in(dir, "").into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn delete_in(dir: &Path, name: &str) -> Result<()> {
        let path = Profile::file_of(dir, name).ok_or_else(|| Error::Invalid(format!("no profile named {name:?}")))?;
        Ok(std::fs::remove_file(path)?)
    }

    pub fn rename_in(dir: &Path, old: &str, new: &str) -> Result<PathBuf> {
        let mut p = Profile::find_in(dir, old).ok_or_else(|| Error::Invalid(format!("no profile named {old:?}")))?;
        let old_path = Profile::file_of(dir, &p.name).unwrap_or_else(|| Profile::path_in(dir, &p.name));
        let same_file = old_path == Profile::path_in(dir, new);
        p.name = new.trim().to_string();
        let path = if same_file { p.save_in(dir)? } else { p.save_new_in(dir)? };
        if path != old_path {
            std::fs::remove_file(old_path)?;
        }
        Ok(path)
    }

    /// `base`, or `base 2`, `base 3`… : the first name free in `dir` and not a built-in.
    pub fn unique_name_in(dir: &Path, base: &str) -> String {
        let base = base.trim();
        let free = |n: &str| !is_builtin_name(n) && !Profile::path_in(dir, n).exists();
        if free(base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| free(n)).unwrap_or_default()
    }

    /// Older versions created custom "Gaming" / "Typing" starters, which now
    /// clash with the built-ins: rename them to "<name> (old)". Never deletes.
    pub fn migrate_in(dir: &Path) {
        for p in Profile::list_in(dir, "").into_iter().filter(|p| is_builtin_name(&p.name)) {
            let mut q = p.clone();
            q.name = Profile::unique_name_in(dir, &format!("{} (old)", p.name));
            let old_path = Profile::file_of(dir, &p.name);
            match q.save_new_in(dir) {
                Ok(_) => {
                    if let Some(old) = old_path {
                        let _ = std::fs::remove_file(old);
                    }
                }
                Err(e) => log::warn!("migrating profile {:?}: {e}", p.name),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hfd::tests::{FakeBoard, f75};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aether-prof-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        d
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
        assert_eq!(loaded, Profile { format: 2, ..mine });
        loaded.apply(&mut kb).unwrap();
        let t = kb.actuation_table().unwrap();
        assert_eq!((t.record(34).trip, t.record(49).trip), (250, 0));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn builtins_set_actuation_only() {
        let mut kb = f75(FakeBoard::new());
        let b = Profile::builtins(&kb.board().slug.clone(), &kb.layout().clone());
        let names: Vec<&str> = b.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Firmware default", "Competitive", "Gaming", "Balanced", "Typing"]);
        assert!(b.iter().all(|p| p.builtin && p.lighting.is_none() && p.effect.is_none()));

        let gaming = b[2].actuation_table().unwrap();
        let w = gaming.record(34);
        assert_eq!((w.trip, w.press, w.release, w.rapid_trigger()), (80, 15, 15, true));
        assert!(gaming.record(13).is_empty(), "slots that aren't keys stay empty");
        assert_eq!(b[0].actuation_table().unwrap(), ActTable::default());

        let static_mode = aether_proto::Lighting { mode: 1, ..kb.lighting().unwrap() };
        kb.set_lighting(&static_mode).unwrap();
        b[2].apply(&mut kb).unwrap();
        assert_eq!(kb.actuation_table().unwrap(), gaming);
        assert_eq!(kb.lighting().unwrap(), static_mode, "built-ins leave lighting alone");
    }

    #[test]
    fn format_1_files_still_load() {
        let dir = tmp("v1");
        std::fs::create_dir_all(&dir).unwrap();
        let v1 = format!(
            r#"{{"format":1,"name":"Old","board":"aula-f75-he","lighting":{{"mode":1,"rgb":[1,2,3],"alpha":255,"color_mode":0,"brightness":5,"speed":3,"reserved":[0,0,0]}},"actuation":"{}"}}"#,
            "00".repeat(aether_proto::ACT_TABLE_SIZE)
        );
        std::fs::write(dir.join("old.json"), v1).unwrap();
        let p = Profile::find_in(&dir, "old").unwrap();
        assert_eq!(p.lighting.unwrap().rgb, [1, 2, 3]);
        assert!(p.effect.is_none() && !p.builtin);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn names_never_clobber() {
        let dir = tmp("nm");
        let mut kb = f75(FakeBoard::new());
        let p = |n: &str, kb: &mut dyn Keyboard| Profile::capture(kb, n).unwrap();
        p("Mine", &mut kb).save_new_in(&dir).unwrap();
        assert!(p("mine", &mut kb).save_new_in(&dir).is_err(), "same name");
        assert!(p("Mine!", &mut kb).save_in(&dir).is_err(), "different profile, same file");
        p("MINE", &mut kb).save_in(&dir).unwrap(); // same profile: update
        assert!(p("Gaming", &mut kb).save_in(&dir).is_err(), "built-in name");
        assert_eq!(Profile::unique_name_in(&dir, "Mine"), "Mine 2");
        assert_eq!(Profile::unique_name_in(&dir, "typing"), "typing 2");

        Profile::rename_in(&dir, "mine", "Office").unwrap();
        assert!(Profile::find_in(&dir, "Office").is_some() && Profile::find_in(&dir, "Mine").is_none());
        p("Home", &mut kb).save_new_in(&dir).unwrap();
        assert!(Profile::rename_in(&dir, "Office", "home").is_err(), "no clobbering");
        assert!(Profile::rename_in(&dir, "Office", "Balanced").is_err());
        Profile::rename_in(&dir, "Office", "office").unwrap(); // case-only rename
        assert_eq!(Profile::find_in(&dir, "OFFICE").unwrap().name, "office");
        Profile::delete_in(&dir, "OFFICE").unwrap();
        assert_eq!(Profile::list_in(&dir, "").len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn oddly_named_files_can_be_renamed_and_deleted() {
        let dir = tmp("odd");
        let mut kb = f75(FakeBoard::new());
        let p = Profile::capture(&mut kb, "Default").unwrap();
        p.write(&dir, &dir.join("Default.json")).unwrap();
        p.save_in(&dir).unwrap();
        assert_eq!(Profile::list_in(&dir, "").len(), 1, "updates the old file, no duplicate");
        Profile::rename_in(&dir, "default", "Home").unwrap();
        assert!(!dir.join("Default.json").exists());
        Profile::delete_in(&dir, "home").unwrap();
        assert!(Profile::list_in(&dir, "").is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn old_starters_are_renamed_not_lost() {
        let dir = tmp("mg");
        let mut kb = f75(FakeBoard::new());
        let mut g = Profile::capture(&mut kb, "x").unwrap();
        g.name = "Gaming".into();
        g.write(&dir, &Profile::path_in(&dir, "Gaming")).unwrap();
        Profile::migrate_in(&dir);
        let names: Vec<String> = Profile::list_in(&dir, "").into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Gaming (old)"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn listing_filters_board_and_wrong_board_is_refused() {
        let dir = tmp("bd");
        let mut kb = f75(FakeBoard::new());
        let mut p = Profile::capture(&mut kb, "x").unwrap();
        p.board = "some-other-board".into();
        p.save_in(&dir).unwrap();
        assert!(Profile::list_in(&dir, "aula-f75-he").is_empty());
        assert_eq!(Profile::list_in(&dir, "").len(), 1);
        assert!(p.apply(&mut kb).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
