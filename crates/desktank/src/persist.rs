//! The app's saved tank (SPEC §2.6, T8): load at start, autosave every
//! [`AUTOSAVE`] while the tank runs, save when it is hidden and on exit.
//!
//! The format is `desktank_sim::save`; the files, ownership lock and
//! atomic writes are `desktank_platform::SaveStore`. A second process that
//! finds the tank owned shows the newest save read-only and never writes.
//! Saves are checked from the event loop's existing wake-ups, so saving
//! adds no timers: a hidden tank is paused and doesn't change.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use desktank_platform::{SaveStore, data_dir};
use desktank_sim::{MaterialTable, World, save};

/// Time between autosaves while the tank runs.
pub const AUTOSAVE: Duration = Duration::from_mins(5);
/// The one tank the window shows (per-display tanks come with O4).
const TANK: &str = "main";

/// The saved tank and when it was last written.
pub struct Persistence {
    store: SaveStore,
    /// The world tick last saved (or loaded): nothing to save until it moves.
    saved_tick: Option<u64>,
    next_autosave: Instant,
}

impl Persistence {
    /// Opens the store in `dir` (default: the data directory's `tanks`)
    /// and tries to take ownership of the tank.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no data directory or it can't be
    /// created.
    pub fn open(dir: Option<PathBuf>) -> anyhow::Result<Self> {
        let dir = match dir {
            Some(dir) => dir,
            None => data_dir()
                .context("no data directory (HOME / APPDATA unset)")?
                .join("tanks"),
        };
        let mut store = SaveStore::open(&dir, TANK)
            .with_context(|| format!("opening the save directory {}", dir.display()))?;
        if !store.try_lock().context("locking the tank")? {
            eprintln!(
                "desktank: another DeskTank owns the tank in {}; showing its last save \
                 read-only, changes won't be saved",
                dir.display()
            );
        }
        Ok(Self {
            store,
            saved_tick: None,
            next_autosave: Instant::now() + AUTOSAVE,
        })
    }

    /// The newest save that loads, if any. Saves that don't load are
    /// logged, and moved aside if this process owns the tank.
    pub fn load(&mut self) -> Option<World> {
        let loaded = match self.store.load(|bytes| {
            let materials = MaterialTable::builtin().map_err(|e| e.to_string())?;
            save::decode(bytes, materials).map_err(|e| e.to_string())
        }) {
            Ok(loaded) => loaded,
            Err(e) => {
                eprintln!(
                    "desktank: error: listing saves in {}: {e}",
                    self.store.dir().display()
                );
                return None;
            }
        };
        for (path, why) in &loaded.damaged {
            let action = if self.store.owns() {
                "moved aside"
            } else {
                "skipped"
            };
            eprintln!(
                "desktank: error: save {} doesn't load ({why}); {action}",
                path.display()
            );
        }
        let (world, path) = loaded.value?;
        eprintln!(
            "desktank: loaded {} (tick {})",
            path.display(),
            world.tick_count()
        );
        self.saved_tick = Some(world.tick_count());
        Some(world)
    }

    /// Saves if the autosave is due and the world moved on.
    pub fn autosave(&mut self, world: &World, now: Instant) {
        if now >= self.next_autosave {
            self.save(world, "autosave");
        }
    }

    /// Saves now if the world moved on since the last save (and this
    /// process owns the tank). Errors are logged; the earlier saves stay.
    pub fn save(&mut self, world: &World, why: &str) {
        self.next_autosave = Instant::now() + AUTOSAVE;
        if !self.store.owns() || self.saved_tick == Some(world.tick_count()) {
            return;
        }
        let t0 = Instant::now();
        let bytes = save::encode(world);
        let encoded = t0.elapsed();
        match self.store.write(&bytes) {
            Ok(path) => {
                self.saved_tick = Some(world.tick_count());
                eprintln!(
                    "desktank: {why}: {} ({} KiB, encoded in {:.1} ms, written in {:.1} ms)",
                    path.display(),
                    bytes.len().div_ceil(1024),
                    encoded.as_secs_f64() * 1e3,
                    t0.elapsed().saturating_sub(encoded).as_secs_f64() * 1e3,
                );
            }
            Err(e) => eprintln!("desktank: error: {why} failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::WorldConfig;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("desktank-persist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn world(seed: u64) -> World {
        let config = WorldConfig {
            width: 96,
            height: 64,
            seed,
            ..WorldConfig::default()
        };
        let mut w = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        crate::scene::starter(&mut w, seed).unwrap();
        w
    }

    #[test]
    fn saves_only_when_the_world_moved_on() {
        let dir = temp("moved");
        let mut p = Persistence::open(Some(dir.clone())).unwrap();
        assert!(p.load().is_none());
        let mut w = world(1);
        w.tick();
        p.save(&w, "test");
        p.save(&w, "test");
        let count = || std::fs::read_dir(&dir).unwrap().count();
        // One save plus the lock file.
        assert_eq!(count(), 2);
        w.tick();
        p.autosave(&w, Instant::now());
        assert_eq!(count(), 2, "not due yet");
        p.autosave(&w, Instant::now() + AUTOSAVE);
        assert_eq!(count(), 3);
        drop(p);
        let mut p = Persistence::open(Some(dir.clone())).unwrap();
        assert_eq!(p.load().unwrap().state_hash(), w.state_hash());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_save_is_moved_aside_for_a_fresh_tank() {
        let dir = temp("damaged");
        let mut p = Persistence::open(Some(dir.clone())).unwrap();
        let mut w = world(2);
        w.tick();
        p.save(&w, "test");
        drop(p);
        // Damage the only save.
        let save = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "dtank"))
            .unwrap();
        let mut bytes = std::fs::read(&save).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        std::fs::write(&save, bytes).unwrap();

        let mut p = Persistence::open(Some(dir.clone())).unwrap();
        assert!(p.load().is_none(), "a fresh tank");
        assert!(!save.exists());
        assert!(save.with_extension("dtank.damaged").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_second_process_reads_but_never_writes() {
        let dir = temp("second");
        let mut owner = Persistence::open(Some(dir.clone())).unwrap();
        let mut w = world(3);
        w.tick();
        owner.save(&w, "test");
        let mut viewer = Persistence::open(Some(dir.clone())).unwrap();
        let mut seen = viewer.load().unwrap();
        seen.tick();
        viewer.save(&seen, "test");
        // Still only the owner's save.
        assert_eq!(viewer.load().unwrap().tick_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
