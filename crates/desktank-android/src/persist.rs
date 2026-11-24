//! The Dream's shared tank (SPEC §3.6), using the desktop save store and format.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use desktank_platform::SaveStore;
use desktank_sim::{MaterialTable, World, save};

use crate::{NewWorldError, TvGrid, new_world};

/// Time between autosaves while a tank is drawing.
pub const AUTOSAVE: Duration = Duration::from_mins(5);
const TANK: &str = "main";

/// Saved tank, ownership lock and autosave deadline.
pub struct Persistence {
    store: SaveStore,
    saved_tick: Option<u64>,
    next_autosave: Duration,
}

impl Persistence {
    /// Opens the app's tank store at `files_dir/tanks` and tries to take ownership.
    /// Times are measured from the render thread's monotonic clock origin.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be opened or taking the lock fails.
    pub fn open(files_dir: &Path, now: Duration) -> io::Result<Self> {
        let mut store = SaveStore::open(files_dir.join("tanks"), TANK)?;
        if !store.try_lock()? {
            log::warn!(
                "another process owns the tank in {}; loading read-only",
                store.dir().display()
            );
        }
        Ok(Self {
            store,
            saved_tick: None,
            next_autosave: now + AUTOSAVE,
        })
    }

    /// Loads the newest decodable save and logs any rejected files.
    pub fn load(&mut self) -> Option<World> {
        let loaded = match self.store.load(decode) {
            Ok(loaded) => loaded,
            Err(e) => {
                log::warn!(
                    "could not list saves in {}: {e}",
                    self.store.dir().display()
                );
                return None;
            }
        };
        for (path, error) in loaded.damaged {
            log::warn!("could not load save {}: {error}", path.display());
        }
        let (world, path) = loaded.value?;
        self.saved_tick = Some(world.tick_count());
        log::debug!("loaded save {}", path.display());
        Some(world)
    }

    /// Saves if the autosave is due and the world has advanced.
    pub fn autosave(&mut self, world: &World, now: Duration) {
        if now >= self.next_autosave {
            self.save(world, "autosave", now);
        }
    }

    /// Saves a new tick if this process owns the tank, and resets the deadline.
    pub fn save(&mut self, world: &World, why: &str, now: Duration) {
        self.next_autosave = now + AUTOSAVE;
        if !self.store.owns() || self.saved_tick == Some(world.tick_count()) {
            return;
        }
        let started = Instant::now();
        let bytes = save::encode(world);
        match self.store.write(&bytes) {
            Ok(path) => {
                self.saved_tick = Some(world.tick_count());
                log::info!(
                    "{why}: {} ({} KiB, {:.1} ms)",
                    path.display(),
                    bytes.len().div_ceil(1024),
                    started.elapsed().as_secs_f64() * 1e3,
                );
            }
            Err(e) => log::warn!("{why} failed in {}: {e}", self.store.dir().display()),
        }
    }
}

fn decode(bytes: &[u8]) -> Result<World, String> {
    let materials = MaterialTable::builtin().map_err(|e| e.to_string())?;
    save::decode(bytes, materials).map_err(|e| e.to_string())
}

/// Restores the shared tank with its saved grid or stocks a new tank for `grid`.
///
/// # Errors
///
/// Returns an error if a new world is needed and cannot be created.
pub fn start_world(
    persist: Option<&mut Persistence>,
    grid: TvGrid,
) -> Result<World, NewWorldError> {
    if let Some(world) = persist.and_then(Persistence::load) {
        log::info!(
            "loaded tank {}x{} cells, tick {}",
            world.width(),
            world.height(),
            world.tick_count()
        );
        return Ok(world);
    }
    let world = new_world(grid)?;
    log::info!(
        "new tank {}x{} cells, tick {}",
        world.width(),
        world.height(),
        world.tick_count()
    );
    Ok(world)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use desktank_sim::{WorldConfig, scene};

    use super::*;
    use crate::tv_grid;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "desktank-android-persist-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn falls_back_to_the_stocked_tank_without_a_valid_save() {
        let dir = TempDir::new("fallback");
        let grid = tv_grid(640, 360);
        let expected = new_world(grid).unwrap().state_hash();
        let mut persist = Persistence::open(&dir.0, Duration::ZERO).unwrap();
        assert_eq!(
            start_world(Some(&mut persist), grid).unwrap().state_hash(),
            expected,
        );

        persist.store.write(b"damaged").unwrap();
        assert_eq!(
            start_world(Some(&mut persist), grid).unwrap().state_hash(),
            expected,
        );
    }

    #[test]
    fn loads_the_newest_valid_desktop_save_with_its_original_grid() {
        let dir = TempDir::new("desktop");
        let config = WorldConfig {
            width: 96,
            height: 64,
            seed: 42,
            ..WorldConfig::default()
        };
        let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        scene::starter(&mut world, 42).unwrap();

        let mut store = SaveStore::open(dir.0.join("tanks"), "main").unwrap();
        assert!(store.try_lock().unwrap());
        store.write(&save::encode(&world)).unwrap();
        world.tick();
        store.write(&save::encode(&world)).unwrap();
        store.write(b"damaged").unwrap();
        drop(store);

        let mut persist = Persistence::open(&dir.0, Duration::ZERO).unwrap();
        let loaded = start_world(Some(&mut persist), tv_grid(1920, 1080)).unwrap();
        assert_eq!((loaded.width(), loaded.height()), (96, 64));
        assert_eq!(loaded.state_hash(), world.state_hash());
        persist.save(&loaded, "unchanged after loading", Duration::ZERO);
        assert!(!dir.0.join("tanks/main-0000000004.dtank").exists());
    }

    #[test]
    fn saves_changed_ticks_in_the_shared_format_and_autosaves_when_due() {
        let dir = TempDir::new("autosave");
        let mut persist = Persistence::open(&dir.0, Duration::ZERO).unwrap();
        let mut world = new_world(tv_grid(640, 360)).unwrap();
        world.tick();
        let tanks = dir.0.join("tanks");
        let first = tanks.join("main-0000000001.dtank");
        let second = tanks.join("main-0000000002.dtank");
        let third = tanks.join("main-0000000003.dtank");

        persist.autosave(
            &world,
            AUTOSAVE.checked_sub(Duration::from_secs(1)).unwrap(),
        );
        assert!(!first.exists());
        persist.autosave(&world, AUTOSAVE);
        let bytes = fs::read(first).unwrap();
        let decoded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
        assert_eq!(decoded.state_hash(), world.state_hash());

        persist.save(&world, "stopped", AUTOSAVE);
        assert!(!second.exists());
        world.tick();
        persist.autosave(&world, AUTOSAVE * 2 + Duration::from_secs(1));
        assert!(second.exists());
        persist.autosave(&world, AUTOSAVE * 4);
        assert!(!third.exists());

        drop(persist);
        let mut reopened = Persistence::open(&dir.0, Duration::ZERO).unwrap();
        assert_eq!(reopened.load().unwrap().state_hash(), world.state_hash());
    }
}
