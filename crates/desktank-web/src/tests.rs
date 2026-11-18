use super::*;

const MS: f64 = 1.0;
const PRIMARY: u32 = BUTTON_PRIMARY;
const SECONDARY: u32 = BUTTON_SECONDARY;

/// 128×72 cells in a 512×288 canvas: scale 4, no letterbox.
fn tank() -> WebTank {
    WebTank::new(128, 72, 7, 512, 288).unwrap()
}

/// A paused tank, so only the tools change cells.
fn paused() -> WebTank {
    let mut t = tank();
    t.set_running(false);
    t
}

/// Canvas pixel at the centre of cell `(x, y)`.
fn px(t: &WebTank, x: i32, y: i32) -> (f64, f64) {
    let p = t.pixel_at(x, y);
    (p[0], p[1])
}

fn down(t: &mut WebTank, button: u32, x: i32, y: i32) -> u32 {
    let (px, py) = px(t, x, y);
    t.pointer_down(button, px, py)
}

fn drag(t: &mut WebTank, x: i32, y: i32) -> u32 {
    let (px, py) = px(t, x, y);
    t.pointer_move(px, py)
}

fn slot(t: &WebTank, entry: &str) -> u32 {
    let i = t.palette().iter().position(|e| e == entry);
    u32::try_from(i.unwrap_or_else(|| panic!("{entry} not in the palette"))).unwrap()
}

#[test]
fn grid_scale_is_the_largest_giving_320_across() {
    assert_eq!(grid_for(1920, 1080, 0), [6, 320, 180]);
    assert_eq!(grid_for(1896, 1066, 0), [5, 379, 213]);
    assert_eq!(grid_for(640, 360, 0), [2, 320, 180]);
    assert_eq!(grid_for(639, 359, 0), [1, 639, 359]);
    assert_eq!(grid_for(200, 100, 0), [1, 200, 100], "narrow: scale 1");
    assert_eq!(grid_for(5120, 2880, 0), [8, 640, 360], "scale capped at 8");
    assert_eq!(grid_for(0, 0, 0), [1, 1, 1]);
}

#[test]
fn grid_scale_override_is_clamped() {
    assert_eq!(grid_for(1920, 1080, 3), [3, 640, 360]);
    assert_eq!(grid_for(1920, 1080, 99), [8, 240, 135]);
    assert_eq!(grid_for(4096, 100, 1), [1, MAX_GRID_SIDE, 100]);
}

#[test]
fn rejects_bad_sizes() {
    assert!(WebTank::new(0, 10, 0, 1, 1).is_err());
    assert!(WebTank::new(MAX_GRID_SIDE + 1, 10, 0, 1, 1).is_err());
}

#[test]
fn lists_builtin_materials() {
    let names = tank().materials();
    for m in ["water", "air", "sand", "gravel", "rock", "air_bubble"] {
        assert!(names.iter().any(|n| n == m), "{m} missing");
    }
}

/// The page's palette is the sim's: every pour material, every tool and
/// every device, with a colour for each material and device.
#[test]
fn palette_matches_the_sim() {
    let t = tank();
    let palette = t.palette();
    assert_eq!(palette.len(), Tools::palette(t.tank().world()).len());
    for m in tools::POUR_MATERIALS {
        assert!(palette.contains(&format!("material {m}")), "{m}");
    }
    for tool in ["scoop", "glue", "net", "inspect"] {
        assert!(palette.contains(&format!("tool {tool}")), "{tool}");
    }
    for p in desktank_sim::Placeable::ALL {
        assert!(palette.contains(&format!("device {}", p.material_name())));
    }
    let colours = t.palette_colours();
    assert_eq!(colours.len(), palette.len());
    for (entry, colour) in palette.iter().zip(&colours) {
        let is_tool = entry.starts_with("tool ");
        assert_eq!(colour.is_empty(), is_tool, "{entry}: {colour:?}");
        assert!(is_tool || (colour.len() == 7 && colour.starts_with('#')));
    }
    assert_eq!(colours[0], "#d8c08a", "sand's colour from its TOML");
    assert_eq!(t.selected(), 0, "starts on sand");
}

#[test]
fn select_changes_the_tool_and_ignores_bad_indices() {
    let mut t = tank();
    let gravel = slot(&t, "material gravel");
    t.select(gravel);
    assert_eq!(t.selected(), gravel);
    t.select(9999);
    assert_eq!(t.selected(), gravel);
}

#[test]
fn pour_tool_fills_the_cell_under_the_pointer() {
    let mut t = paused();
    assert_eq!(t.material_at(40, 30), "water");
    assert!(down(&mut t, PRIMARY, 40, 30) > 0);
    t.pointer_up(PRIMARY);
    assert_eq!(t.material_at(40, 30), "sand");
}

#[test]
fn a_fast_drag_leaves_no_gaps() {
    let mut t = paused();
    t.set_radius(1);
    down(&mut t, PRIMARY, 10, 30);
    drag(&mut t, 110, 30);
    t.pointer_up(PRIMARY);
    for x in 10..=110 {
        assert_eq!(t.material_at(x, 30), "sand", "column {x}");
    }
}

#[test]
fn secondary_button_scoops_whatever_is_selected() {
    let mut t = paused();
    down(&mut t, PRIMARY, 40, 30);
    t.pointer_up(PRIMARY);
    assert_eq!(t.material_at(40, 30), "sand");
    assert!(down(&mut t, SECONDARY, 40, 30) > 0);
    t.pointer_up(SECONDARY);
    assert_eq!(t.material_at(40, 30), "water");
    // Above the surface, scoop leaves air.
    let scoop = slot(&t, "tool scoop");
    t.select(0);
    down(&mut t, PRIMARY, 10, 2);
    t.pointer_up(PRIMARY);
    t.select(scoop);
    assert!(down(&mut t, PRIMARY, 10, 2) > 0);
    t.pointer_up(PRIMARY);
    assert_eq!(t.material_at(10, 2), "air");
}

#[test]
fn glue_toggles_a_poured_rock() {
    let mut t = paused();
    t.select(slot(&t, "material rock"));
    down(&mut t, PRIMARY, 60, 40);
    drag(&mut t, 70, 40);
    t.pointer_up(PRIMARY);
    t.step();
    assert_eq!(t.glued_bodies(), 0);
    t.select(slot(&t, "tool glue"));
    assert_eq!(down(&mut t, PRIMARY, 65, 40), 1, "one body glued");
    t.pointer_up(PRIMARY);
    assert_eq!(t.glued_bodies(), 1);
    assert_eq!(down(&mut t, PRIMARY, 65, 40), 1, "and freed again");
    t.pointer_up(PRIMARY);
    assert_eq!(t.glued_bodies(), 0);
}

#[test]
fn a_rock_stroke_makes_one_body_until_release() {
    let mut t = paused();
    t.select(slot(&t, "material rock"));
    down(&mut t, PRIMARY, 20, 30);
    for x in (24..60).step_by(4) {
        drag(&mut t, x, 30);
    }
    t.pointer_up(PRIMARY);
    t.step();
    assert_eq!(t.tank().world().bodies().count(), 1);
    down(&mut t, PRIMARY, 100, 30);
    t.pointer_up(PRIMARY);
    t.step();
    assert_eq!(t.tank().world().bodies().count(), 2);
}

#[test]
fn devices_are_placed_once_per_click() {
    let mut t = paused();
    t.select(slot(&t, "device bubbler"));
    assert_eq!(down(&mut t, PRIMARY, 64, 60), 8);
    drag(&mut t, 80, 60);
    t.pointer_up(PRIMARY);
    assert_eq!(t.material_count("bubbler"), 8, "a drag places nothing more");
    assert_eq!(t.material_count("unobtainium"), 0);
}

#[test]
fn inspect_reports_the_cell_and_net_changes_nothing() {
    let mut t = paused();
    t.select(slot(&t, "tool inspect"));
    let hash = t.state_hash();
    assert_eq!(down(&mut t, PRIMARY, 12, 40), 0);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspection(), "cell water 12 40");
    t.select(slot(&t, "tool net"));
    down(&mut t, PRIMARY, 12, 40);
    drag(&mut t, 30, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(t.state_hash(), hash, "net and inspect change no cells");
}

#[test]
fn long_press_inspects_and_keeps_the_selection() {
    let mut t = paused();
    let gravel = slot(&t, "material gravel");
    t.select(gravel);
    let (x, y) = px(&t, 5, 2);
    assert_eq!(t.inspect_at(x, y), "cell air 5 2");
    assert_eq!(t.selected(), gravel);
    assert_eq!(t.material_count("gravel"), 0, "inspecting poured nothing");
}

#[test]
fn held_brush_pours_every_tick() {
    let mut t = tank();
    down(&mut t, PRIMARY, 64, 20);
    let first = t.material_count("sand");
    assert!(first > 0);
    t.frame(0.0);
    for i in 1..10 {
        t.frame(f64::from(i) * 50.0);
    }
    assert!(t.material_count("sand") > first, "kept pouring");
    t.pointer_up(PRIMARY);
    let before = t.material_count("sand");
    t.frame(600.0);
    assert_eq!(t.material_count("sand"), before, "released");
}

#[test]
fn brush_radius_is_clamped_and_shows_an_outline() {
    let mut t = paused();
    assert_eq!(t.radius(), tools::DEFAULT_RADIUS);
    t.set_radius(1000);
    assert_eq!(t.radius(), tools::MAX_RADIUS);
    assert_eq!(t.resize_brush(-1000), tools::MIN_RADIUS);
    assert_eq!(brush_range(), [tools::MIN_RADIUS, tools::MAX_RADIUS]);
    // Hovering asks for a frame (the outline), as does leaving.
    let (x, y) = px(&t, 30, 30);
    t.pointer_move(x, y);
    assert!(t.tank().frame_wanted());
    t.pointer_leave();
    assert!(t.tank().frame_wanted());
}

#[test]
fn a_drag_far_off_the_canvas_is_clamped() {
    let mut t = paused();
    t.set_radius(0);
    down(&mut t, PRIMARY, 64, 30);
    // A billion pixels away: clamped to just outside the grid.
    t.pointer_move(1e9, 120.0);
    t.pointer_up(PRIMARY);
    assert_eq!(t.material_at(127, 30), "sand");
}

#[test]
fn feed_sprinkles_food_below_the_surface() {
    let mut t = paused();
    assert_eq!(t.feed(), FEED_PINCHES);
    assert_eq!(t.material_count(FEED_FOOD), FEED_PINCHES);
}

#[test]
fn field_overlay_round_trips() {
    let mut t = tank();
    assert_eq!(t.field_overlay(), "");
    t.set_field_overlay("co2").unwrap();
    assert_eq!(t.field_overlay(), "co2");
    assert!(t.set_field_overlay("magic").is_err());
    t.set_field_overlay("").unwrap();
    assert_eq!(t.field_overlay(), "");
}

#[test]
fn frame_ticks_at_20_hz_and_pauses() {
    let mut t = tank();
    let ticks = |f: u32| f >> FRAME_TICKS_SHIFT;
    assert_eq!(ticks(t.frame(0.0)), 0, "the first frame starts the clock");
    assert_eq!(ticks(t.frame(16.0 * MS)), 0);
    assert_eq!(ticks(t.frame(50.0 * MS)), 1);
    assert_eq!(ticks(t.frame(150.0 * MS)), 2);
    t.set_running(false);
    assert_eq!(ticks(t.frame(1000.0 * MS)), 0);
    assert_eq!(ticks(t.frame(2000.0 * MS)), 0, "paused: no world time");
    t.step();
    assert_eq!(t.tick_count(), 4.0);
    t.set_running(true);
    assert_eq!(ticks(t.frame(2000.0 * MS)), 0, "resuming doesn't catch up");
    assert_eq!(ticks(t.frame(2050.0 * MS)), 1);
}

#[test]
fn wake_at_is_the_next_tick() {
    let mut t = tank();
    t.frame(0.0);
    t.frame(60.0);
    // Without graphics a frame stays wanted, so the wake is the frame cap
    // or the tick, whichever is first; never later than the next tick.
    assert!(
        t.wake_at() > 60.0 && t.wake_at() <= 100.0,
        "{}",
        t.wake_at()
    );
}

#[test]
fn a_frame_is_wanted_after_an_edit() {
    let mut t = tank();
    // Without graphics nothing is drawn, so the first frame stays wanted.
    assert_ne!(t.frame(0.0) & FRAME_WANTED, 0);
    assert_eq!(t.frame(0.0) & FRAME_DREW, 0);
}

#[test]
fn maps_canvas_pixels_to_cells_with_letterbox() {
    let mut t = tank();
    // 512×288 holds 128×72 at scale 4 exactly.
    assert_eq!(t.cell_at(0.0, 0.0), [0, 0]);
    assert_eq!(t.cell_at(511.0, 287.0), [127, 71]);
    assert_eq!(t.pixel_at(0, 0), [2.0, 2.0]);
    // A wider canvas letterboxes left and right at scale 4.
    t.resize(600, 288);
    assert_eq!(t.cell_at(44.0, 0.0), [0, 0]);
    assert_eq!(t.cell_at(43.0, 0.0), [-1, 0]);
    assert_eq!(t.pixel_at(0, 0), [46.0, 2.0]);
}

#[test]
fn script_matches_native_capture_world() {
    let src = include_str!("../../desktank/tests/scenes/layers.toml");
    let t = WebTank::from_script(src, 7, 320, 180).unwrap();
    let script = Script::parse(src).unwrap();
    let mut world = World::new(script.world_config(7), builtin().unwrap()).unwrap();
    script.run(&mut world, script.ticks).unwrap();
    assert_eq!(t.tank().world().state_hash(), world.state_hash());
    assert_eq!(t.tick_count(), 160.0);
    assert!(!t.running(), "a scripted scene starts paused");
    let starter = include_str!("../../desktank/tests/scenes/starter.toml");
    assert!(WebTank::from_script(starter, 7, 320, 180).is_err());
}

#[test]
fn same_seed_same_world() {
    let run = || {
        let mut t = tank();
        t.select(slot(&t, "material gravel"));
        down(&mut t, PRIMARY, 30, 10);
        t.pointer_up(PRIMARY);
        for _ in 0..50 {
            t.step();
        }
        t.state_hash()
    };
    assert_eq!(run(), run());
}
