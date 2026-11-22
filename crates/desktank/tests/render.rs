//! Render tests (SPEC §7): `desktank capture` of each scene in
//! `tests/scenes/` is compared with `tests/reference/<scene>.png`. A test
//! passes when at most 0.5% of pixels differ by more than CIEDE2000 ΔE 2.
//!
//! Each scene is also captured with the split grid (`--split-grid`, the
//! webOS build's smaller shaders in more passes), which must match the
//! one-pass capture within ΔE 2 at every pixel.
//!
//! Update the references deliberately with
//! `DESKTANK_BLESS=1 cargo test -p desktank --test render`.
//!
//! Without a GPU adapter the tests are skipped locally, but fail when `CI`
//! or `DESKTANK_BLESS` is set: Linux CI renders on Mesa lavapipe, macOS on Metal and Windows on
//! WARP.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Scenes are 160×90 cells drawn at scale 2.
const SIZE: &str = "320x180";
/// The same scenes at scale 4, where plants show their four-texel detail.
const SIZE_X4: &str = "640x360";
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

/// Captures `scene` at `size` (lit for `time`, `HH:MM`, if given, with
/// `extra` arguments) into `name` and returns the PNG path, or `None` if
/// there is no GPU adapter outside CI.
fn capture(
    scene: &str,
    name: &str,
    time: Option<&str>,
    size: &str,
    extra: &[&str],
) -> Option<PathBuf> {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    let script = manifest()
        .join("tests/scenes")
        .join(format!("{scene}.toml"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_desktank"));
    command.args(["capture", "--seed", SEED, "--size", size, "--script"]);
    command.arg(&script);
    if let Some(t) = time {
        command.args(["--time", t]);
    }
    command.args(extra);
    let output = command.arg("--out").arg(&out).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        if stderr.contains("no suitable GPU adapter")
            && std::env::var_os("CI").is_none()
            && std::env::var_os("DESKTANK_BLESS").is_none()
        {
            eprintln!("skipping {scene}: {stderr}");
            return None;
        }
        panic!("desktank capture failed for {scene}: {stderr}");
    }
    Some(out.join("frame.png"))
}

fn check(scene: &str) {
    check_named(scene, scene, None, SIZE);
}

/// Checks `scene` lit for `time` (`HH:MM`) against `<scene>_HHMM.png`.
fn check_at(scene: &str, time: &str) {
    let name = format!("{scene}_{}", time.replace(':', ""));
    check_named(scene, &name, Some(time), SIZE);
}

fn check_named(scene: &str, name: &str, time: Option<&str>, size: &str) {
    let reference = manifest()
        .join("tests/reference")
        .join(format!("{name}.png"));
    let Some(got_path) = capture(scene, name, time, size, &[]) else {
        return;
    };
    if std::env::var_os("DESKTANK_BLESS").is_some() {
        std::fs::copy(&got_path, &reference).unwrap();
        eprintln!("blessed {}", reference.display());
        return;
    }
    compare_with(name, &got_path, &reference, MAX_FRACTION);
    check_split(scene, name, time, size, &[], &got_path);
}

/// Captures `scene` with the split grid and asserts it matches `one_pass`,
/// the same capture drawn in one pass, within ΔE 2 at every pixel: the
/// passes differ only by rounding where a translucent plant texel blends.
fn check_split(
    scene: &str,
    name: &str,
    time: Option<&str>,
    size: &str,
    extra: &[&str],
    one_pass: &Path,
) {
    let args: Vec<&str> = extra.iter().copied().chain(["--split-grid"]).collect();
    let Some(split) = capture(scene, &format!("{name}_split"), time, size, &args) else {
        return;
    };
    compare_with(&format!("{name} (split grid)"), &split, one_pass, 0.0);
}

/// Asserts the capture at `got_path` matches `reference`: at most
/// `max_fraction` of pixels differ by more than ΔE 2.
fn compare_with(name: &str, got_path: &Path, reference: &Path, max_fraction: f64) {
    let (gw, gh, got) = read_png(got_path);
    let (rw, rh, want) = read_png(reference);
    assert_eq!(
        (gw, gh),
        (rw, rh),
        "{name}: size differs from the reference"
    );
    let diff = support::compare(&got, &want, MAX_DELTA_E);
    eprintln!(
        "{name}: {} of {} pixels over ΔE {MAX_DELTA_E} ({:.3}%), max ΔE {:.2}",
        diff.over,
        diff.total,
        diff.fraction() * 100.0,
        diff.max
    );
    assert!(
        diff.fraction() <= max_fraction,
        "{name}: {:.3}% of pixels differ by more than ΔE {MAX_DELTA_E} (max ΔE {:.2}); \
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

/// Day and night (T6): the planted tank at night, dawn, midday and dusk.
#[test]
fn planted_scene_matches_reference_at_four_times_of_day() {
    for time in ["00:00", "06:30", "12:00", "20:30"] {
        check_at("planted", time);
    }
}

/// Plant species (T6d): all seven (rotala, hygrophila, ludwigia, cabomba,
/// bacopa, java moss and christmas moss) side by side, each drawn with its
/// own shapes and colour ramp.
#[test]
fn plant_species_scene_matches_reference() {
    check("plant_species");
}

/// T6d: a starving plant. Rotala shaded by rock slabs at staggered ticks:
/// healthy, then dying (brown and translucent), browning, yellowing and
/// pale.
#[test]
fn plant_starving_scene_matches_reference() {
    check("plant_starving");
}

/// T6d: a strongly lit red plant. Rotala and ludwigia on a terrace just
/// under the surface colour pink-red in lean aquasoil water; a rotala on
/// the low bed stays green.
#[test]
fn plant_lit_scene_matches_reference() {
    check("plant_lit");
}

/// T6c: amazon sword (on a root tab), vallisneria and cryptocoryne grown
/// for 15 minutes, with runners and runner-rooted plants.
#[test]
fn plant_rosettes_scene_matches_reference() {
    check("plant_rosettes");
}

/// T6c: dwarf hairgrass and Monte Carlo carpets grown for 15 minutes.
#[test]
fn plant_carpets_scene_matches_reference() {
    check("plant_carpets");
}

/// T6c part 2: duckweed and frogbit floating at the surface, grown for 8
/// minutes, the frogbit shading a stem plant below it.
#[test]
fn plant_floaters_scene_matches_reference() {
    check("plant_floaters");
}

/// T6c part 3: anubias on rock, java fern on driftwood, layered christmas
/// moss, branching rotala and surface-trailing cabomba, grown for 15
/// minutes.
#[test]
fn plant_epiphytes_scene_matches_reference() {
    check("plant_epiphytes");
}

/// Issue #45: tiger lotus (mottled leaf pads on petioles of differing
/// lengths, the longest floating on the surface) and marimo balls resting
/// on the bed, grown for 25 minutes.
#[test]
fn plant_marimo_lotus_scene_matches_reference() {
    check("plant_marimo_lotus");
}

/// Four-texel plant detail: the plant scenes at scale 4, where every leaf,
/// stem and root shape is drawn with quarter-cell structure, against
/// `<scene>_x4.png`.
#[test]
fn plant_scenes_match_reference_at_scale_4() {
    for scene in [
        "plant_species",
        "plant_starving",
        "plant_lit",
        "plant_rosettes",
        "plant_carpets",
        "plant_floaters",
        "plant_epiphytes",
        "plant_marimo_lotus",
    ] {
        check_named(scene, &format!("{scene}_x4"), None, SIZE_X4);
    }
}

/// Devices (D27, DV1): internal filters on both side glasses, drawn as
/// sprites (the right one mirrored) over cells drawn as water.
#[test]
fn devices_scene_matches_reference() {
    check("devices");
}

/// Creatures (T7a): sprites of every species over the starter bed, after
/// they have swum for 30 s and come for food.
#[test]
fn creatures_scene_matches_reference() {
    check("creatures");
}

/// Turned crawlers (T7b): nerites with their feet on a rock pillar, under
/// its overhanging arm and on both panes of glass.
#[test]
fn climbing_scene_matches_reference() {
    check("climbing");
}

/// Shrimp and crabs: cherry and amano shrimp on an aquasoil bed grazing
/// algae, and Thai micro crabs on the plants and rock.
#[test]
fn invertebrates_scene_matches_reference() {
    check("invertebrates");
}

/// Live food: a pinch of live bloodworms wriggling on the starter bed by a
/// few frozen ones, with a harlequin coming over; at scale 4 too, for the
/// worms' fine frames.
#[test]
fn live_food_scene_matches_reference() {
    check("live_food");
    check_named("live_food", "live_food_x4", None, SIZE_X4);
}

/// More fish: a harlequin rasbora school, male guppies and honey gouramis
/// come up for flakes over the starter bed, and otocinclus on a rock, the
/// glass and the bed.
#[test]
fn fish_scene_matches_reference() {
    check("fish");
}

/// Life stages and corpses (T9): guppies from fry to adult, an elder tetra
/// drawn greyer, a corpse rotting on the bed and one sinking belly up.
#[test]
fn lifecycle_scene_matches_reference() {
    check("lifecycle");
}

/// Breeding (T9 part 2): cherry shrimp in each morph, guppy pairs in three
/// strains (males and the plainer, short-tailed females), and a corydoras
/// clutch of eggs on the bed.
#[test]
fn breeding_scene_matches_reference() {
    check("breeding");
}

/// The breeding scene at scale 4: each morph and the female guppy drawn
/// from their recoloured four-texel art, against `breeding_x4.png`.
#[test]
fn breeding_scene_matches_reference_at_scale_4() {
    check_named("breeding", "breeding_x4", None, SIZE_X4);
}

/// 2.5D depth, part 2 (D21): fish swimming through a dense planted stand
/// at scale 4. Plant cells nearer the glass hide the fish behind them,
/// through the gaps their shapes leave; fish further back fade towards the
/// water.
#[test]
fn depth_scene_matches_reference_at_scale_4() {
    check_named("depth", "depth_x4", None, SIZE_X4);
}

/// The cheat-only blinker at scale 4, drawn from its four-texel art: by
/// day, and at midnight with its green night glow (rim, eye whites and a
/// soft halo), against `blinker_x4.png` and `blinker_x4_0000.png`.
#[test]
fn blinker_scene_matches_reference_by_day_and_night() {
    check_named("blinker", "blinker_x4", None, SIZE_X4);
    check_named("blinker", "blinker_x4_0000", Some("00:00"), SIZE_X4);
}

/// The split grid with what only some frames show: swaying plants (a
/// neighbour's leaf drawn over a cell, or a cell's own leaf swayed away)
/// at both plant-art levels, and the field overlay and night tint, which
/// the plant passes apply before blending over the ground.
#[test]
fn split_grid_matches_one_pass_with_sway_and_overlays() {
    for (scene, size, time, extra) in [
        ("depth", SIZE_X4, None, &["--sway"][..]),
        (
            "plant_floaters",
            SIZE,
            Some("21:00"),
            &["--sway", "--field", "flow"][..],
        ),
        (
            "plant_rosettes",
            SIZE_X4,
            None,
            &["--sway", "--field", "light"][..],
        ),
    ] {
        let name = format!("{scene}_sway");
        let Some(one_pass) = capture(scene, &name, time, size, extra) else {
            return;
        };
        check_split(scene, &name, time, size, extra, &one_pass);
    }
}
