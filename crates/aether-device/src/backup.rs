//! Snapshot of the board settings Aether can change, written before every
//! change and restorable byte-for-byte.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_proto::{ActTable, Lighting};
use serde::{Deserialize, Serialize};

use crate::{Error, Keyboard, Result};

/// `~/.local/share/aether-he/backups`
pub fn backups_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "aether-he")
        .ok_or_else(|| Error::Data("no home directory".into()))?;
    Ok(dirs.data_local_dir().join("backups"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backup {
    pub format: u32,
    pub created_unix: u64,
    pub board: String,
    pub lighting: SavedLighting,
    /// The whole actuation table, hex.
    pub actuation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedLighting {
    pub mode: u8,
    pub rgb: [u8; 3],
    pub alpha: u8,
    pub color_mode: u8,
    pub brightness: u8,
    pub speed: u8,
    pub reserved: [u8; 3],
}

impl From<Lighting> for SavedLighting {
    fn from(l: Lighting) -> Self {
        let Lighting { mode, rgb, alpha, color_mode, brightness, speed, reserved } = l;
        SavedLighting { mode, rgb, alpha, color_mode, brightness, speed, reserved }
    }
}

impl From<SavedLighting> for Lighting {
    fn from(s: SavedLighting) -> Self {
        let SavedLighting { mode, rgb, alpha, color_mode, brightness, speed, reserved } = s;
        Lighting { mode, rgb, alpha, color_mode, brightness, speed, reserved }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        return Err(Error::Data("odd-length hex".into()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| Error::Data(e.to_string())))
        .collect()
}

impl Backup {
    pub fn capture(kb: &mut dyn Keyboard) -> Result<Backup> {
        let lighting = kb.lighting()?.into();
        let table = kb.actuation_table()?;
        Ok(Backup {
            format: 1,
            created_unix: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            board: kb.board().slug.clone(),
            lighting,
            actuation: hex(&table.0),
        })
    }

    pub fn actuation_table(&self) -> Result<ActTable> {
        Ok(ActTable::from_slice(&unhex(&self.actuation)?)?)
    }

    /// Save as `<board>-<unix time>.json` in `dir`.
    /// Never overwrites: two backups in the same second get consecutive
    /// timestamps, so the oldest/latest order stays exact.
    pub fn save_in(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let mut b = self.clone();
        loop {
            let path = dir.join(format!("{}-{}.json", b.board, b.created_unix));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    use std::io::Write;
                    let text = serde_json::to_string_pretty(&b).map_err(|e| Error::Data(e.to_string()))?;
                    f.write_all((text + "\n").as_bytes())?;
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => b.created_unix += 1,
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|e| Error::Data(e.to_string()))?;
        std::fs::write(path, text + "\n")?;
        Ok(())
    }

    /// Capture and save to [`backups_dir`]; returns the file written.
    pub fn snapshot(kb: &mut dyn Keyboard) -> Result<PathBuf> {
        Backup::capture(kb)?.save_in(&backups_dir()?)
    }

    pub fn load(path: &Path) -> Result<Backup> {
        let text = std::fs::read_to_string(path)?;
        let b: Backup = serde_json::from_str(&text).map_err(|e| Error::Data(format!("{}: {e}", path.display())))?;
        b.actuation_table()?; // validate now, not halfway through a restore
        Ok(b)
    }

    /// Write lighting and actuation back, then verify the actuation table.
    pub fn restore(&self, kb: &mut dyn Keyboard) -> Result<()> {
        if self.board != kb.board().slug {
            return Err(Error::Invalid(format!(
                "backup is for {}, connected board is {}",
                self.board,
                kb.board().slug
            )));
        }
        let table = self.actuation_table()?;
        kb.set_lighting(&self.lighting.into())?;
        kb.write_actuation_table(&table)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        if kb.actuation_table()? != table {
            return Err(Error::BadReply("actuation read-back differs from the backup".into()));
        }
        Ok(())
    }

    /// The newest backup for `board` in [`backups_dir`].
    pub fn latest(board: &str) -> Result<Option<PathBuf>> {
        Ok(list_in(&backups_dir()?, board).pop().map(|(_, p)| p))
    }

    /// The oldest backup for `board`: the keyboard as it was before Aether
    /// first changed anything, since every change backs up first.
    pub fn oldest(board: &str) -> Result<Option<PathBuf>> {
        Ok(list_in(&backups_dir()?, board).into_iter().next().map(|(_, p)| p))
    }

    /// All backups in [`backups_dir`] (any board), oldest first, with their time.
    pub fn list() -> Result<Vec<(u64, PathBuf)>> {
        Ok(list_in(&backups_dir()?, ""))
    }
}

/// `<board>-<unix>.json` files in `dir` for `board` ("" = any), oldest first.
/// Sorted by the number, not the name, so the order can't depend on digit count.
fn list_in(dir: &Path, board: &str) -> Vec<(u64, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<(u64, PathBuf)> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?;
            let (name, ts) = stem.rsplit_once('-')?;
            let ok = p.extension()? == "json" && (board.is_empty() || name == board);
            ok.then(|| ts.parse().ok().map(|t| (t, p.clone())))?
        })
        .collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hfd::tests::{FakeBoard, f75};
    use aether_proto::Actuation;

    #[test]
    fn listing_orders_by_time_and_filters_board() {
        let dir = std::env::temp_dir().join(format!("aether-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["aula-f75-he-1792590510.json", "aula-f75-he-999999999.json", "aula-f75-he-1792591950.json",
                     "other-board-5.json", "aula-f75-he-notes.txt", "aula-f75-he-x.json"] {
            std::fs::write(dir.join(name), "{}").unwrap();
        }
        let times: Vec<u64> = list_in(&dir, "aula-f75-he").into_iter().map(|(t, _)| t).collect();
        assert_eq!(times, [999999999, 1792590510, 1792591950], "numeric, not lexical, order");
        assert_eq!(list_in(&dir, "").len(), 4);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn same_second_backups_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("aether-same-{}", std::process::id()));
        let mut b = Backup::capture(&mut f75(FakeBoard::new())).unwrap();
        b.created_unix = 1_000;
        let p1 = b.save_in(&dir).unwrap();
        b.actuation = "00".repeat(aether_proto::ACT_TABLE_SIZE - 1) + "07";
        let p2 = b.save_in(&dir).unwrap();
        assert_ne!(p1, p2);
        assert_ne!(Backup::load(&p1).unwrap().actuation, Backup::load(&p2).unwrap().actuation);
        let order: Vec<PathBuf> = list_in(&dir, &b.board).into_iter().map(|(_, p)| p).collect();
        assert_eq!(order, [p1, p2], "the later save sorts last");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn backup_restore_round_trip() {
        let mut kb = f75(FakeBoard::new());
        let b = Backup::capture(&mut kb).unwrap();
        let dir = std::env::temp_dir().join(format!("aether-test-{}", std::process::id()));
        let path = b.save_in(&dir).unwrap();
        assert_eq!(Backup::load(&path).unwrap(), b);

        kb.set_actuation(&[34], Actuation::Fixed { travel_mm: 2.0 }).unwrap();
        let static_mode = Lighting { mode: 1, ..kb.lighting().unwrap() };
        kb.set_lighting(&static_mode).unwrap();
        b.restore(&mut kb).unwrap();
        assert_eq!(Backup::capture(&mut kb).unwrap().actuation, b.actuation);
        assert_eq!(SavedLighting::from(kb.lighting().unwrap()), b.lighting);
        std::fs::remove_dir_all(dir).ok();
    }
}
