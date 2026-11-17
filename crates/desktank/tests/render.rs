//! Render tests (SPEC §7): `desktank capture` of each scene in
//! `tests/scenes/` is compared with `tests/reference/<scene>.png`. A test
//! passes when at most 0.5% of pixels differ by more than CIEDE2000 ΔE 2.
//!
//! Update the references deliberately with
//! `DESKTANK_BLESS=1 cargo test -p desktank --test render`.
//!
//! Without a GPU adapter the tests are skipped locally, but fail when `CI`
//! is set: Linux CI renders on Mesa lavapipe, macOS on Metal and Windows on
//! WARP.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Scenes are 160×90 cells drawn at scale 2.
const SIZE: &str = "320x180";
const SEED: &str = "7";
const MAX_DELTA_E: f64 = 2.0;
const MAX_FRACTION: f64 = 0.005;

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba, "{}", path.display());
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// Captures `scene` and returns the PNG path, or `None` if there is no GPU
/// adapter outside CI.
fn capture(scene: &str) -> Option<PathBuf> {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render")
        .join(scene);
    let _ = std::fs::remove_dir_all(&out);
    let script = manifest()
        .join("tests/scenes")
        .join(format!("{scene}.toml"));
    let output = Command::new(env!("CARGO_BIN_EXE_desktank"))
        .args(["capture", "--seed", SEED, "--size", SIZE, "--script"])
        .arg(&script)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        if stderr.contains("no suitable GPU adapter") && std::env::var_os("CI").is_none() {
            eprintln!("skipping {scene}: {stderr}");
            return None;
        }
        panic!("desktank capture failed for {scene}: {stderr}");
    }
    Some(out.join("frame.png"))
}

fn check(scene: &str) {
    let Some(got_path) = capture(scene) else {
        return;
    };
    let reference = manifest()
        .join("tests/reference")
        .join(format!("{scene}.png"));
    if std::env::var_os("DESKTANK_BLESS").is_some() {
        std::fs::copy(&got_path, &reference).unwrap();
        eprintln!("blessed {}", reference.display());
        return;
    }
    let (gw, gh, got) = read_png(&got_path);
    let (rw, rh, want) = read_png(&reference);
    assert_eq!(
        (gw, gh),
        (rw, rh),
        "{scene}: size differs from the reference"
    );
    let diff = support::compare(&got, &want, MAX_DELTA_E);
    eprintln!(
        "{scene}: {} of {} pixels over ΔE {MAX_DELTA_E} ({:.3}%), max ΔE {:.2}",
        diff.over,
        diff.total,
        diff.fraction() * 100.0,
        diff.max
    );
    assert!(
        diff.fraction() <= MAX_FRACTION,
        "{scene}: {:.3}% of pixels differ by more than ΔE {MAX_DELTA_E} (max ΔE {:.2}); \
         capture at {}, reference at {}; if the change is intended, rerun with DESKTANK_BLESS=1",
        diff.fraction() * 100.0,
        diff.max,
        got_path.display(),
        reference.display()
    );
}

#[test]
fn starter_scene_matches_reference() {
    check("starter");
}

#[test]
fn layers_scene_matches_reference() {
    check("layers");
}
