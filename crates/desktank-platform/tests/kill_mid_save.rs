//! SPEC T8 acceptance: killing the process mid-save leaves a loadable
//! previous save, and the lock passes to the next process.
//!
//! The test runs its own binary again as a child that holds the lock and
//! writes large saves back to back, kills it at varied moments, and checks
//! that a new owner loads an intact save every time.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use desktank_platform::{KEEP, SaveStore};

const CHILD_DIR: &str = "DESKTANK_KILL_TEST_DIR";
/// Big enough that a write takes a while, so kills land mid-write.
const SAVE_BYTES: usize = 8 << 20;
/// Kills, at a different moment each.
const ROUNDS: u64 = 10;

/// A self-checking save: the generation, then a fill byte, repeated.
fn payload(k: u64) -> Vec<u8> {
    let mut v = k.to_le_bytes().to_vec();
    #[allow(clippy::cast_possible_truncation)] // a fill byte
    v.resize(SAVE_BYTES, k as u8);
    v.extend_from_slice(&k.to_le_bytes());
    v
}

fn check(bytes: &[u8]) -> Result<u64, String> {
    let n = bytes.len();
    if n != SAVE_BYTES + 8 {
        return Err(format!("{n} bytes"));
    }
    let k = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    if bytes[n - 8..] != k.to_le_bytes() || payload(k) != bytes {
        return Err("torn".to_owned());
    }
    Ok(k)
}

/// The child: owns the store and writes saves until it is killed.
#[test]
fn child_writer() {
    let Some(dir) = std::env::var_os(CHILD_DIR) else {
        return; // only runs as the child
    };
    let mut store = SaveStore::open(PathBuf::from(dir), "main").unwrap();
    assert!(store.try_lock().unwrap());
    let next = store.load(check).unwrap().value.map_or(1, |(k, _)| k + 1);
    let mut k = next;
    loop {
        store.write(&payload(k)).unwrap();
        k += 1;
    }
}

fn spawn(dir: &Path) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_writer", "--nocapture", "--test-threads=1"])
        .env(CHILD_DIR, dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn killing_mid_save_leaves_a_loadable_save() {
    let dir = std::env::temp_dir().join(format!("desktank-kill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut last = 0;
    let mut mid_write = 0;
    for round in 0..ROUNDS {
        let mut child = spawn(&dir);
        // Wait for it to own the tank and write past the last round's save.
        let mut viewer = SaveStore::open(&dir, "main").unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(Instant::now() < deadline, "the child never saved");
            let newest = viewer.load(check).unwrap().value.map(|(k, _)| k);
            if newest.is_some_and(|k| k > last) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!viewer.try_lock().unwrap(), "the writer owns the tank");
        // Kill at a varied point within the next write.
        std::thread::sleep(Duration::from_millis(3 + 5 * round));
        child.kill().unwrap();
        child.wait().unwrap();

        // The lock is free again, and the newest intact save loads.
        if dir.join("main.tmp").exists() {
            mid_write += 1;
        }
        let mut owner = SaveStore::open(&dir, "main").unwrap();
        assert!(
            owner.try_lock().unwrap(),
            "round {round}: lock not released"
        );
        let loaded = owner.load(check).unwrap();
        assert!(
            loaded.damaged.is_empty(),
            "round {round}: {:?}",
            loaded.damaged
        );
        let (k, _) = loaded.value.expect("a save loads");
        assert!(k > last, "round {round}");
        last = k;
        assert!(!dir.join("main.tmp").exists(), "stray temp file removed");
        let saves = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".dtank")
            })
            .count();
        assert!((1..=KEEP + 1).contains(&saves), "{saves} saves");
    }
    // Usually most rounds; not asserted, as a slow disk could shift it.
    eprintln!("{mid_write} of {ROUNDS} kills landed mid-write");
    std::fs::remove_dir_all(dir).unwrap();
}
