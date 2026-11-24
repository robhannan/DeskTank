//! The app's saved tank (SPEC §2.6, T8): load at start, autosave every
//! [`AUTOSAVE`] while the tank runs, save when it is hidden and on exit.
//!
//! The format is `desktank_sim::save`; the files, ownership lock and
//! atomic writes are `desktank_platform::SaveStore`. A second process that
//! finds the tank owned shows the newest save read-only and never writes.
//! Saves are checked from the event loop's existing wake-ups, so saving
//! adds no timers: a hidden tank is paused and doesn't change.
//!
//! [`import`] and [`export`] move saves between machines and the web
//! version (WB3), which uses the same format.

use std::path::{Path, PathBuf};
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
        let mut store = store(dir)?;
        if !store.try_lock().context("locking the tank")? {
            eprintln!(
                "desktank: another DeskTank owns the tank in {}; showing its last save \
                 read-only, changes won't be saved",
                store.dir().display()
            );
        }
        Ok(Self {
            store,
            saved_tick: None,
            next_autosave: Instant::now() + AUTOSAVE,
        })
    }

    /// The newest save that loads and its path, if any. Saves that don't load are
    /// logged, and moved aside if this process owns the tank.
    pub fn load(&mut self) -> Option<(World, PathBuf)> {
        let loaded = match self.store.load(decode) {
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
        Some((world, path))
    }

    /// Whether this process owns the tank.
    pub fn owns(&self) -> bool {
        self.store.owns()
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

/// The tank's save store in `dir` (default: the data directory's `tanks`).
fn store(dir: Option<PathBuf>) -> anyhow::Result<SaveStore> {
    let dir = match dir {
        Some(dir) => dir,
        None => data_dir()
            .context("no data directory (HOME / APPDATA unset)")?
            .join("tanks"),
    };
    SaveStore::open(&dir, TANK)
        .with_context(|| format!("opening the save directory {}", dir.display()))
}

fn decode(bytes: &[u8]) -> Result<World, String> {
    let materials = MaterialTable::builtin().map_err(|e| e.to_string())?;
    save::decode(bytes, materials).map_err(|e| e.to_string())
}

/// `desktank import`: checks that `file` is a save this build loads and
/// writes it as the tank's newest save, so the next start opens it. The
/// older saves stay (the newest 3 are kept, as always).
///
/// # Errors
///
/// Returns an error if the file can't be read or isn't a loadable save, or
/// a running DeskTank owns the tank.
pub fn import(dir: Option<PathBuf>, file: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let world = decode(&bytes)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("{} doesn't load", file.display()))?;
    let mut store = store(dir)?;
    if !store.try_lock().context("locking the tank")? {
        anyhow::bail!("DeskTank is running and owns the tank; quit it, then import again");
    }
    let path = store.write(&bytes).context("writing the save")?;
    eprintln!(
        "desktank: imported {} ({}×{} cells, tick {}) as {}",
        file.display(),
        world.width(),
        world.height(),
        world.tick_count(),
        path.display()
    );
    Ok(())
}

/// `desktank export`: copies the tank's newest save that loads to `file`.
/// It only reads, so it works while the app runs (the copy is then as of
/// the app's last save).
///
/// # Errors
///
/// Returns an error if there is no loadable save or `file` can't be
/// written.
pub fn export(dir: Option<PathBuf>, file: &Path) -> anyhow::Result<()> {
    let store = store(dir)?;
    let loaded = store
        .load(|bytes| decode(bytes).map(|_| bytes.to_vec()))
        .context("listing saves")?;
    let (bytes, from) = loaded.value.context("there is no save to export yet")?;
    std::fs::write(file, &bytes).with_context(|| format!("writing {}", file.display()))?;
    eprintln!(
        "desktank: exported {} to {}",
        from.display(),
        file.display()
    );
    Ok(())
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
        desktank_sim::scene::starter(&mut w, seed).unwrap();
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
        assert_eq!(p.load().unwrap().0.state_hash(), w.state_hash());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saves_catch_up_without_a_normal_tick() {
        let dir = temp("catch-up");
        let mut p = Persistence::open(Some(dir.clone())).unwrap();
        p.save(&world(1), "test");

        let (mut w, original) = p.load().unwrap();
        w.catch_up(60);
        p.save(&w, "test");

        let (loaded, latest) = p.load().unwrap();
        assert_ne!(latest, original);
        assert_eq!(loaded.tick_count(), w.tick_count());
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
        assert!(owner.owns());
        assert!(!viewer.owns());
        let (mut seen, _) = viewer.load().unwrap();
        seen.tick();
        viewer.save(&seen, "test");
        // Still only the owner's save.
        assert_eq!(viewer.load().unwrap().0.tick_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_then_import_moves_a_tank() {
        let from = temp("export");
        let to = temp("import");
        let mut p = Persistence::open(Some(from.clone())).unwrap();
        let mut w = world(4);
        for _ in 0..30 {
            w.tick();
        }
        p.save(&w, "test");
        let file = from.join("out.dtank");
        // Export only reads, so it works beside the owner.
        export(Some(from.clone()), &file).unwrap();
        import(Some(to.clone()), &file).unwrap();
        let mut q = Persistence::open(Some(to.clone())).unwrap();
        assert_eq!(q.load().unwrap().0.state_hash(), w.state_hash());
        // Not a save: rejected, nothing written.
        std::fs::write(&file, b"not a save").unwrap();
        drop(q);
        assert!(import(Some(to.clone()), &file).is_err());
        // The owner blocks an import.
        let _owner = Persistence::open(Some(to.clone())).unwrap();
        export(Some(from.clone()), &file).unwrap();
        assert!(import(Some(to.clone()), &file).is_err());
        let empty = temp("empty");
        assert!(export(Some(empty.clone()), &file).is_err());
        std::fs::remove_dir_all(empty).unwrap();
        std::fs::remove_dir_all(from).unwrap();
        std::fs::remove_dir_all(to).unwrap();
    }
}
