//! Where tanks are saved, and how (SPEC §2.6, T8): one directory per
//! store, an ownership lock, atomic writes that keep the last
//! [`KEEP`] saves, and damaged saves moved aside.
//!
//! The store deals in bytes; the save format (envelope, checksum, world
//! snapshot) is `desktank_sim::save`, passed in as the decoder.
//!
//! **Files** for a tank called `main`:
//!
//! - `main-0000000042.dtank`: saves, numbered by a generation counter; the
//!   highest is the newest.
//! - `main.tmp`: a save being written. A write goes to this file, is
//!   flushed to disk (`fsync`), then renamed to the next generation, so a
//!   process killed mid-save leaves only a stray temporary file and every
//!   numbered save intact. The owner removes the stray file when it takes
//!   the lock.
//! - `main-0000000041.dtank.damaged`: a save that failed to load, moved
//!   aside by the owner so it isn't tried again (and kept for inspection).
//! - `main.lock`: the ownership lock (an OS advisory lock, released if the
//!   process dies). The spec puts the lock on the save file, but saves are
//!   replaced by renames, which would swap the locked file out from under
//!   the lock, so it lives on a file of its own.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Saves kept per tank; older ones are deleted after each write.
pub const KEEP: usize = 3;
/// Extension of a save file.
const EXT: &str = "dtank";
/// Suffix added to a save that failed to load.
const DAMAGED: &str = "damaged";

/// The saves of one tank in one directory.
#[derive(Debug)]
pub struct SaveStore {
    dir: PathBuf,
    name: String,
    /// The lock file, held while this process owns the tank.
    lock: Option<File>,
}

/// What [`SaveStore::load`] found.
#[derive(Debug)]
pub struct Loaded<T> {
    /// The newest save that decoded, and its path; `None` if there was no
    /// save, or none decoded.
    pub value: Option<(T, PathBuf)>,
    /// Saves that failed to load, newest first: the path (after it was
    /// moved aside, if this process owns the tank) and why it failed.
    pub damaged: Vec<(PathBuf, String)>,
}

impl SaveStore {
    /// The store for tank `name` (letters, digits, `-` and `_`) in `dir`,
    /// creating the directory if needed. It doesn't take the lock.
    ///
    /// # Errors
    ///
    /// Returns an error if the name is unusable or the directory can't be
    /// created.
    pub fn open(dir: impl Into<PathBuf>, name: &str) -> io::Result<Self> {
        let ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bad tank name {name:?}"),
            ));
        }
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            name: name.to_owned(),
            lock: None,
        })
    }

    /// The directory the saves live in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Tries to take ownership of the tank. Returns `false` if another
    /// process (or another store in this one) holds it. Taking it removes a
    /// temporary file a killed writer left behind.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock file can't be opened or locked for a
    /// reason other than being held.
    pub fn try_lock(&mut self) -> io::Result<bool> {
        if self.lock.is_some() {
            return Ok(true);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(format!("{}.lock", self.name)))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(e)) => return Err(e),
        }
        self.lock = Some(file);
        match fs::remove_file(self.tmp_path()) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        Ok(true)
    }

    /// Whether this store holds the tank's lock.
    #[must_use]
    pub fn owns(&self) -> bool {
        self.lock.is_some()
    }

    /// Releases the lock (also released when the store is dropped).
    pub fn unlock(&mut self) {
        self.lock = None;
    }

    /// Loads the newest save that `decode` accepts. Saves it rejects are
    /// reported, and moved aside if this store owns the tank; an older save
    /// is tried next.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory can't be listed.
    pub fn load<T, E: core::fmt::Display>(
        &self,
        mut decode: impl FnMut(&[u8]) -> Result<T, E>,
    ) -> io::Result<Loaded<T>> {
        let mut damaged = Vec::new();
        for (_, path) in self.saves()?.into_iter().rev() {
            let why = match fs::read(&path) {
                Ok(bytes) => match decode(&bytes) {
                    Ok(value) => {
                        return Ok(Loaded {
                            value: Some((value, path)),
                            damaged,
                        });
                    }
                    Err(e) => e.to_string(),
                },
                Err(e) => e.to_string(),
            };
            let path = if self.owns() {
                let aside = path.with_extension(format!("{EXT}.{DAMAGED}"));
                fs::rename(&path, &aside).map(|()| aside).unwrap_or(path)
            } else {
                path
            };
            damaged.push((path, why));
        }
        Ok(Loaded {
            value: None,
            damaged,
        })
    }

    /// Writes a save atomically as the newest generation and deletes all
    /// but the newest [`KEEP`]. Returns its path.
    ///
    /// # Errors
    ///
    /// Returns an error if this store doesn't own the tank, or writing the
    /// new save fails; the earlier saves are then untouched.
    pub fn write(&self, bytes: &[u8]) -> io::Result<PathBuf> {
        if !self.owns() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "another process owns this tank",
            ));
        }
        let tmp = self.tmp_path();
        {
            let mut f = File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        // Moved-aside saves count too, so a number is never reused.
        let next = self.generations()?.last().map_or(1, |&(g, _)| g + 1);
        let path = self.path(next);
        fs::rename(&tmp, &path)?;
        sync_dir(&self.dir)?;
        // The new save is safe on disk; pruning is best effort (a viewer
        // may be reading an old one), and the next write tries again.
        if let Ok(saves) = self.saves() {
            for (_, old) in &saves[..saves.len().saturating_sub(KEEP)] {
                let _ = fs::remove_file(old);
            }
        }
        Ok(path)
    }

    /// The saves, oldest first.
    fn saves(&self) -> io::Result<Vec<(u64, PathBuf)>> {
        Ok(self
            .generations()?
            .into_iter()
            .filter(|(_, damaged)| !damaged)
            .map(|(g, _)| (g, self.path(g)))
            .collect())
    }

    /// Every generation on disk, ascending, and whether it was moved aside.
    fn generations(&self) -> io::Result<Vec<(u64, bool)>> {
        let prefix = format!("{}-", self.name);
        let mut found = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let name = entry?.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(rest) = name.strip_prefix(&prefix) else {
                continue;
            };
            let (number, damaged) = match rest.split_once('.') {
                Some((n, ext)) if ext == EXT => (n, false),
                Some((n, ext)) if ext == format!("{EXT}.{DAMAGED}") => (n, true),
                _ => continue,
            };
            if number.len() == 10
                && let Ok(g) = number.parse::<u64>()
            {
                found.push((g, damaged));
            }
        }
        found.sort_unstable();
        Ok(found)
    }

    fn path(&self, generation: u64) -> PathBuf {
        self.dir
            .join(format!("{}-{generation:010}.{EXT}", self.name))
    }

    fn tmp_path(&self) -> PathBuf {
        self.dir.join(format!("{}.tmp", self.name))
    }
}

/// Flushes a rename in `dir` to disk. Windows can't open a directory as a
/// file; NTFS journals the rename itself.
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// The per-user directory DeskTank keeps its data in: `DESKTANK_DATA_DIR`
/// if set, else `~/Library/Application Support/DeskTank` (macOS),
/// `%APPDATA%\DeskTank` (Windows) or `$XDG_DATA_HOME/desktank` (falling
/// back to `~/.local/share/desktank`) elsewhere. `None` if the
/// environment names no home.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("DESKTANK_DATA_DIR") {
        return Some(dir);
    }
    if cfg!(target_os = "macos") {
        Some(var("HOME")?.join("Library/Application Support/DeskTank"))
    } else if cfg!(windows) {
        Some(var("APPDATA")?.join("DeskTank"))
    } else {
        var("XDG_DATA_HOME")
            .or_else(|| Some(var("HOME")?.join(".local/share")))
            .map(|d| d.join("desktank"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("desktank-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn decode(bytes: &[u8]) -> Result<Vec<u8>, String> {
        if bytes.first() == Some(&b'!') {
            Err("damaged".to_owned())
        } else {
            Ok(bytes.to_vec())
        }
    }

    #[test]
    fn keeps_the_newest_three() {
        let dir = temp("keep");
        let mut s = SaveStore::open(&dir, "main").unwrap();
        assert!(s.write(b"x").is_err(), "writes need the lock");
        assert!(s.try_lock().unwrap());
        assert!(s.load(decode).unwrap().value.is_none());
        for k in 0..5u8 {
            s.write(&[k]).unwrap();
        }
        let names: Vec<_> = s.saves().unwrap().into_iter().map(|(g, _)| g).collect();
        assert_eq!(names, [3, 4, 5]);
        let loaded = s.load(decode).unwrap();
        assert_eq!(loaded.value.unwrap().0, [4]);
        assert_eq!(loaded.damaged, Vec::new());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn moves_damaged_saves_aside_and_falls_back() {
        let dir = temp("damaged");
        let mut s = SaveStore::open(&dir, "main").unwrap();
        assert!(s.try_lock().unwrap());
        s.write(b"good").unwrap();
        s.write(b"!bad").unwrap();
        let loaded = s.load(decode).unwrap();
        assert_eq!(loaded.value.unwrap().0, b"good");
        assert_eq!(loaded.damaged.len(), 1);
        assert!(
            loaded.damaged[0]
                .0
                .to_string_lossy()
                .ends_with(".dtank.damaged")
        );
        // Moved aside: not tried again, and its number isn't reused.
        assert_eq!(s.load(decode).unwrap().damaged, Vec::new());
        assert!(s.write(b"next").unwrap().ends_with("main-0000000003.dtank"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_viewer_leaves_damaged_saves_in_place() {
        let dir = temp("viewer");
        let mut owner = SaveStore::open(&dir, "main").unwrap();
        assert!(owner.try_lock().unwrap());
        owner.write(b"!bad").unwrap();
        let mut viewer = SaveStore::open(&dir, "main").unwrap();
        assert!(!viewer.try_lock().unwrap(), "one owner at a time");
        let loaded = viewer.load(decode).unwrap();
        assert!(loaded.value.is_none());
        assert!(loaded.damaged[0].0.to_string_lossy().ends_with(".dtank"));
        owner.unlock();
        assert!(viewer.try_lock().unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_bad_names() {
        let dir = temp("names");
        assert!(SaveStore::open(&dir, "../x").is_err());
        assert!(SaveStore::open(&dir, "").is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
