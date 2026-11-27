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
fn grid_scale_is_the_largest_even_giving_320_across() {
    assert_eq!(grid_for(1920, 1080, 0), [6, 320, 180]);
    assert_eq!(grid_for(1896, 1066, 0), [4, 474, 266], "5 rounds down to 4");
    assert_eq!(grid_for(640, 360, 0), [2, 320, 180]);
    assert_eq!(grid_for(639, 359, 0), [1, 639, 359]);
    assert_eq!(grid_for(200, 100, 0), [1, 200, 100], "narrow: scale 1");
    assert_eq!(grid_for(5120, 2880, 0), [8, 640, 360], "scale capped at 8");
    assert_eq!(grid_for(0, 0, 0), [1, 1, 1]);
}

#[test]
fn grid_scale_override_is_clamped() {
    assert_eq!(grid_for(1920, 1080, 4), [4, 480, 270]);
    assert_eq!(
        grid_for(1920, 1080, 3),
        [2, 960, 540],
        "odd overrides round down"
    );
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
    for m in t.tank().world().materials().iter() {
        let listed = palette.contains(&format!("material {}", m.name));
        assert_eq!(listed, m.palette.is_some(), "{}", m.name);
    }
    for tool in ["scoop", "glue", "net", "inspect", "magnify"] {
        assert!(palette.contains(&format!("tool {tool}")), "{tool}");
    }
    for p in desktank_sim::Placeable::ALL {
        assert!(palette.contains(&format!("device {}", p.name())));
    }
    let colours = t.palette_colours();
    assert_eq!(colours.len(), palette.len());
    for (entry, colour) in palette.iter().zip(&colours) {
        let is_tool = entry.starts_with("tool ");
        assert_eq!(colour.is_empty(), is_tool, "{entry}: {colour:?}");
        assert!(is_tool || (colour.len() == 7 && colour.starts_with('#')));
    }
    let sand = slot(&t, "material sand");
    assert_eq!(
        colours[sand as usize], "#d8c08a",
        "sand's colour from its TOML"
    );
    assert_eq!(t.selected(), sand, "starts on sand");
}

/// Each item's sub-category, in the sim's tree order; a sub-category's
/// items are consecutive.
#[test]
fn palette_categories_follow_the_sim() {
    let t = tank();
    let cats = t.palette_categories();
    assert_eq!(cats.len(), t.palette().len());
    let mut order: Vec<&str> = cats.iter().map(String::as_str).collect();
    order.dedup();
    let sim: Vec<_> = Tools::palette_tree(t.tank().world())
        .iter()
        .flat_map(|tab| &tab.subs)
        .map(|s| {
            format!(
                "{}\t{}\t{}",
                s.path.name(),
                s.path.top_label(),
                s.path.label()
            )
        })
        .collect();
    assert_eq!(order, sim, "each sub-category once, in order");
    let cat = |entry: &str| cats[slot(&t, entry) as usize].as_str();
    assert_eq!(cat("material sand"), "build/substrate\tBuild\tSubstrate");
    assert_eq!(cat("tool net"), "tools\tTools\tTools");
    assert_eq!(cat("device bubbler"), "build/devices\tBuild\tDevices");
    assert_eq!(
        cat("device under_gravel_panel"),
        "build/devices\tBuild\tDevices"
    );
    assert_eq!(
        cat("plant java_moss"),
        "plants/moss\tPlants\tMoss & epiphytes"
    );
    assert_eq!(
        cat("creature axolotl"),
        "amphibians\tAmphibians\tAmphibians"
    );
    // Fish are split by stocking role.
    for (fish, sub) in [
        ("neon_tetra", "schooling\tFish\tSchooling"),
        ("betta", "centrepiece\tFish\tCentrepiece"),
        ("fancy_goldfish", "coldwater\tFish\tColdwater"),
        ("otocinclus", "cleanup\tFish\tClean-up crew"),
    ] {
        assert_eq!(cat(&format!("creature {fish}")), format!("fish/{sub}"));
    }
    assert_eq!(
        cat("creature nerite"),
        "invertebrates/snails\tInvertebrates\tSnails"
    );
    // Live food is in Food › Live, not Fish, and not a species the
    // palette stocks.
    assert_eq!(t.live_foods(), ["live_bloodworm", "live_brine_shrimp"]);
    for live in t.live_foods() {
        assert_eq!(cat(&format!("live_food {live}")), "food/live\tFood\tLive");
        assert!(!t.species().contains(&live));
    }
    assert_eq!(cat("material brine_shrimp"), "food/fish\tFood\tFish food");
    // Plant slots carry their species' description for a tooltip.
    let about = t.palette_descriptions();
    assert_eq!(about.len(), cats.len());
    assert_ne!(about[slot(&t, "plant rotala") as usize], "");
    assert_eq!(about[slot(&t, "material sand") as usize], "");
}

#[test]
fn the_propagule_chips_list_the_species_methods_and_seeds_pour() {
    let mut t = tank();
    // Nothing to pick until a plant slot is selected.
    assert_eq!(t.plant_propagules(), Vec::<String>::new());
    assert_eq!(t.plant_propagule(), "");
    assert!(!t.set_plant_propagule("seeds"));
    assert_eq!(t.next_plant_propagule(), "");
    t.select(slot(&t, "plant hygrophila"));
    assert_eq!(
        t.plant_propagules(),
        ["seeds", "cuttings", "fragments", "juvenile", "mature"]
    );
    assert_eq!(t.plant_propagule(), "cuttings");
    assert!(!t.set_plant_propagule("spores"));
    assert!(t.set_plant_propagule("seeds"));
    assert_eq!(t.plant_propagule(), "seeds");
    t.set_radius(0);
    down(&mut t, PRIMARY, 60, 30);
    t.pointer_up(PRIMARY);
    assert_eq!(t.material_count("seed"), 1);
    // Rotala only grows from cuttings: seeds can't be picked. Whole plants
    // follow its one propagule, and the picks wrap.
    t.select(slot(&t, "plant rotala"));
    assert_eq!(t.plant_propagules(), ["cuttings", "juvenile", "mature"]);
    assert_eq!(t.plant_propagule(), "cuttings");
    assert!(!t.set_plant_propagule("seeds"));
    assert_eq!(t.next_plant_propagule(), "juvenile");
    assert_eq!(t.next_plant_propagule(), "mature");
    assert_eq!(t.next_plant_propagule(), "cuttings", "the picks wrap");
    // Carpets spread by fragments and runners (fragments listed first);
    // picked, runners pour a plantlet (the cutting material).
    t.select(slot(&t, "plant dwarf_hairgrass"));
    assert_eq!(
        t.plant_propagules(),
        ["fragments", "runners", "juvenile", "mature"]
    );
    assert_eq!(t.next_plant_propagule(), "runners");
    assert_eq!(t.plant_propagule(), "runners");
    down(&mut t, PRIMARY, 90, 30);
    t.pointer_up(PRIMARY);
    // A plantlet is placed as a small piece of the species' own cells, one
    // per click, not poured as powder (#469).
    assert_eq!(t.material_count("cutting"), 0);
    assert_eq!(t.plant_count(), 1);
    // Floaters spread by fragments and buds (T6c part 2).
    t.select(slot(&t, "plant duckweed"));
    assert_eq!(
        t.plant_propagules(),
        ["fragments", "buds", "juvenile", "mature"]
    );
    assert!(t.set_plant_propagule("buds"));
    assert_eq!(t.plant_propagule(), "buds");
    // Each species keeps its own pick, and a new tank keeps them all.
    t.select(slot(&t, "plant hygrophila"));
    assert_eq!(t.plant_propagule(), "seeds");
    let mut next = tank();
    next.keep_plant_picks(&t);
    next.select(slot(&next, "plant duckweed"));
    assert_eq!(next.plant_propagule(), "buds");
}

#[test]
fn creature_morph_picks_wrap_and_survive_replacement() {
    let mut t = tank();
    t.select(slot(&t, "creature neon_tetra"));
    assert_eq!(t.creature_morphs(), Vec::<String>::new());
    assert!(!t.creature_morph_cycles());
    assert_eq!(t.creature_morph(), "");
    assert!(!t.set_creature_morph(""));
    assert_eq!(t.next_creature_morph(), None);

    t.select(slot(&t, "creature cherry_shrimp"));
    assert_eq!(t.creature_morphs(), ["red", "wild", "blue", "yellow"]);
    assert!(!t.creature_morph_cycles());
    assert_eq!(t.creature_morph(), "red");
    assert!(t.set_creature_morph("yellow"));
    assert_eq!(t.next_creature_morph().as_deref(), Some("red"));
    assert!(t.set_creature_morph("BLUE"));

    t.select(slot(&t, "creature betta"));
    assert!(t.creature_morph_cycles());
    assert_eq!(t.creature_morph(), "");
    assert_eq!(t.next_creature_morph().as_deref(), Some("wild"));
    assert!(t.set_creature_morph("lavender"));
    assert_eq!(t.next_creature_morph().as_deref(), Some(""));
    assert!(t.set_creature_morph("koi"));
    assert!(!t.set_creature_morph("blue"));
    assert_eq!(t.creature_morph(), "koi");
    assert!(t.set_creature_morph(""));
    assert_eq!(t.creature_morph(), "");
    assert!(t.set_creature_morph("copper"));

    let mut next = tank();
    next.select(slot(&next, "creature betta"));
    next.keep_creature_picks(&t);
    assert_eq!(next.creature_morph(), "copper");
    assert_eq!(
        next.tank.card(),
        next.tools
            .selected_creature()
            .map(|id| Card::Creature(id, Some(2)))
    );
    next.select(slot(&next, "creature cherry_shrimp"));
    assert_eq!(next.creature_morph(), "blue");
}

#[test]
fn the_species_card_sits_in_the_bottom_left_nearest_the_dom_palette() {
    let mut t = tank();
    t.select(slot(&t, "creature guppy"));
    let rect = t.card_rect();
    assert_eq!(rect.len(), 4);
    let scale = rect[2] / 144;
    assert_eq!((rect[0], rect[1] + rect[3] + 2 * scale), (2 * scale, 288));
    // A click on it is swallowed, so it closes the card.
    t.pointer_down(PRIMARY, f64::from(rect[0]) + 1.0, f64::from(rect[1]) + 1.0);
    assert_eq!(t.tank.card(), None);
}

#[test]
fn changing_morph_keeps_a_closed_species_card_closed() {
    let mut t = tank();
    t.select(slot(&t, "creature betta"));
    let id = t.tools.selected_creature().unwrap();
    assert!(t.set_creature_morph("koi"));
    assert_eq!(t.tank.card(), Some(Card::Creature(id, Some(3))));
    let rect = t.card_rect();
    t.pointer_down(PRIMARY, f64::from(rect[0]) + 1.0, f64::from(rect[1]) + 1.0);
    assert_eq!(t.tank.card(), None);
    assert_eq!(t.next_creature_morph().as_deref(), Some("marble"));
    assert_eq!(t.tank.card(), None);
    t.select(slot(&t, "creature neon_tetra"));
    t.select(slot(&t, "creature betta"));
    assert_eq!(t.tank.card(), Some(Card::Creature(id, Some(4))));
}

#[test]
fn a_plant_slot_pours_its_species_and_inspect_shows_the_plant() {
    let mut t = tank();
    let plants: Vec<_> = t
        .palette()
        .into_iter()
        .filter_map(|e| e.strip_prefix("plant ").map(str::to_owned))
        .collect();
    assert_eq!(plants.len(), 18, "one slot per species");
    for name in [
        "hygrophila",
        "ludwigia",
        "cabomba",
        "bacopa",
        "java_moss",
        "christmas_moss",
        "amazon_sword",
        "vallisneria",
        "dwarf_hairgrass",
        "monte_carlo",
        "cryptocoryne",
        "duckweed",
        "frogbit",
        "anubias",
        "java_fern",
        "marimo",
        "tiger_lotus",
    ] {
        assert!(plants.iter().any(|s| s == name), "{name}");
    }
    let moss = slot(&t, "plant java_moss");
    t.select(moss);
    assert_eq!(t.selected(), moss);
    t.set_radius(0);
    down(&mut t, PRIMARY, 60, 30);
    t.pointer_up(PRIMARY);
    assert_eq!(t.material_count("cutting"), 0);
    assert_eq!(t.plant_count(), 1);
    let colour = &t.palette_colours()[moss as usize];
    let moss_colour = t
        .tank()
        .world()
        .plant_species()
        .get(desktank_sim::PlantSpeciesId(2));
    let [r, g, b] = moss_colour.unwrap().colour();
    assert_eq!(colour, &format!("#{r:02x}{g:02x}{b:02x}"));
    // A plant made by an edit (the default species), then inspected.
    let leaf = t.tank().world().materials().id("leaf").unwrap();
    t.tank
        .world_mut()
        .fill_rect(
            desktank_sim::Rect {
                x0: 20,
                y0: 40,
                x1: 23,
                y1: 41,
            },
            leaf,
            desktank_sim::Brush::Replace,
        )
        .unwrap();
    // The moss piece placed above is plant 1; this edit made plant 2.
    assert_eq!(t.plant_count(), 2);
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 21, 40);
    t.pointer_up(PRIMARY);
    let line = t.take_inspection();
    let (head, needs) = line.rsplit_once(' ').unwrap();
    assert_eq!(head, "plant hygrophila 2 0 100 3 none 21 40");
    // The plant's care readings, as the card's meters show them: label,
    // percent and level, in the card's order.
    let id = desktank_sim::PlantId(2);
    let readings = t.tank().world().plant_needs(id).unwrap();
    assert_eq!(needs, super::needs_token(&readings));
    let labels: Vec<_> = needs
        .split(',')
        .map(|n| n.split(':').next().unwrap())
        .collect();
    assert_eq!(labels, ["Light", "CO₂", "Nutrients"]);
    for (token, r) in needs.split(',').zip(readings) {
        let parts: Vec<_> = token.split(':').collect();
        assert_eq!(parts[1], r.satisfaction.to_string());
        assert_eq!(parts[2], r.level().name());
    }
}

#[test]
fn a_plant_is_inspected_with_its_age_in_life_days() {
    let mut t = paused();
    let world = t.tank.world_mut();
    let aquasoil = world.materials().id("aquasoil").unwrap();
    let stem = world.materials().id("stem").unwrap();
    let floor = desktank_sim::Rect {
        x0: 0,
        y0: 62,
        x1: 128,
        y1: 72,
    };
    world
        .fill_rect(floor, aquasoil, desktank_sim::Brush::Replace)
        .unwrap();
    world.set_cell(21, 61, stem).unwrap();
    // A day of its life (`LIFE_DAY_TICKS` ticks) and a bit more.
    for _ in 0..desktank_sim::LIFE_DAY_TICKS + 20 {
        world.tick();
    }
    assert_eq!(t.plant_count(), 1);
    t.select(slot(&t, "tool inspect"));
    let base = t.tank().world().plants().next().unwrap().base;
    down(
        &mut t,
        PRIMARY,
        i32::try_from(base.0).unwrap(),
        i32::try_from(base.1).unwrap(),
    );
    t.pointer_up(PRIMARY);
    let line = t.take_inspection();
    let age: Vec<_> = line.split(' ').collect();
    assert_eq!(&age[..2], ["plant", "hygrophila"], "{line}");
    assert_eq!(age[3], "1", "one life day old: {line}");
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
    t.select(slot(&t, "material sand"));
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
    let bubbler_cells = u32::try_from(
        t.tank()
            .world()
            .device_kinds()
            .get(desktank_sim::DeviceKind::Bubbler)
            .cells,
    )
    .unwrap();
    t.select(slot(&t, "device bubbler"));
    assert_eq!(down(&mut t, PRIMARY, 64, 60), bubbler_cells);
    drag(&mut t, 80, 60);
    t.pointer_up(PRIMARY);
    assert_eq!(
        t.material_count("device"),
        bubbler_cells,
        "a drag places nothing more"
    );
    assert_eq!(t.tank().world().devices().count(), 1);
    assert_eq!(t.material_count("unobtainium"), 0);
}

#[test]
fn inspect_reports_the_cell_and_net_changes_nothing() {
    let mut t = paused();
    t.select(slot(&t, "tool inspect"));
    let hash = t.state_hash();
    assert_eq!(down(&mut t, PRIMARY, 12, 40), 0);
    t.pointer_up(PRIMARY);
    assert_eq!(t.take_inspection(), "cell water 12 40");
    assert_eq!(t.take_inspection(), "", "taken once");
    t.select(slot(&t, "tool net"));
    down(&mut t, PRIMARY, 12, 40);
    drag(&mut t, 30, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(t.state_hash(), hash, "net and inspect change no cells");
}

#[test]
fn creatures_are_added_inspected_netted_and_lifted_out() {
    let mut t = paused();
    t.select(slot(&t, "creature neon_tetra"));
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(t.creatures(), 1);
    // Inspect shows its card and the cell under it: an adult just mature
    // and well fed, the first of its species (a male), one morph, not
    // carrying, no name yet.
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(
        t.take_inspection(),
        "creature neon_tetra 1 water 60 40 adult 180 100 100 male - - Hunger:0:critical,Social:0:critical "
    );
    // Naming it is kept (cleaned) and marks the tank for saving.
    let _ = t.save();
    assert_eq!(
        t.name_creature(1, "  Big   Blue "),
        Ok("Big Blue".to_owned())
    );
    assert!(t.save_needed());
    assert!(t.name_creature(9, "Nobody").is_err());
    assert_eq!(t.creature_name(1).as_deref(), Some("Big Blue"));
    assert_eq!(t.creature_name(9), None);
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    assert!(t.take_inspection().ends_with(",Social:0:critical Big Blue"));
    // The net carries it; let go in the water it stays, above it it's out.
    t.select(slot(&t, "tool net"));
    down(&mut t, PRIMARY, 60, 40);
    drag(&mut t, 80, 30);
    assert_eq!(t.pointer_up(PRIMARY), 0);
    assert_eq!(t.creatures(), 1);
    down(&mut t, PRIMARY, 80, 30);
    drag(&mut t, 80, 0);
    assert_eq!(t.pointer_up(PRIMARY), 1, "lifted out");
    assert_eq!(t.creatures(), 0);
    // Adding or removing a creature asks for a frame even while paused.
    assert_ne!(t.frame(0.0) & FRAME_WANTED, 0);
}

#[test]
fn the_name_row_is_clicked_to_name_not_to_turn_the_page() {
    let mut t = paused();
    t.select(slot(&t, "creature neon_tetra"));
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    // A viewer (naming not allowed) has no row: its whole card turns.
    assert_eq!(t.inspect_name_rect(), Vec::<u32>::new());
    assert!(!t.name_row_at(0.0, 0.0));
    t.set_naming_allowed(true);
    let row = t.inspect_name_rect();
    assert_eq!(row.len(), 4);
    let card = t.inspect_rect();
    assert!(row[0] >= card[0] && row[1] > card[1]);
    assert!(row[0] + row[2] <= card[0] + card[2] && row[1] + row[3] < card[1] + card[3]);
    // A click on the row is the page's to handle: the page stays, and
    // nothing else in the tank is touched.
    let hash = t.state_hash();
    let (x, y) = (f64::from(row[0] + 2), f64::from(row[1] + 2));
    assert!(t.name_row_at(x, y));
    assert_eq!(t.pointer_down(PRIMARY, x, y), 0);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 1);
    assert_eq!(t.state_hash(), hash);
    // Editing mirrors the typed text into the card; the same text twice,
    // or blank, is no change of state; saving names the creature.
    assert!(!t.inspect_name_editing());
    t.set_inspect_name_edit(Some("Big Blue".to_owned()));
    assert!(t.inspect_name_editing());
    assert_eq!(t.creature_name(1).as_deref(), Some(""), "not named yet");
    assert_eq!(t.name_creature(1, "Big Blue"), Ok("Big Blue".to_owned()));
    t.set_inspect_name_edit(None);
    assert!(!t.inspect_name_editing());
    // A click elsewhere on the card still turns the page, and the species
    // page has no name row.
    let (x, y) = (f64::from(card[0] + 2), f64::from(card[1] + 2));
    assert!(!t.name_row_at(x, y));
    t.pointer_down(PRIMARY, x, y);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 2);
    assert_eq!(t.inspect_name_rect(), Vec::<u32>::new());
    // Starting to edit on that page shows page one again.
    t.set_inspect_name_edit(Some(String::new()));
    assert_eq!(t.inspect_page(), 1);
    assert_eq!(t.inspect_name_rect().len(), 4);
    // Another tool removes the card and the edit with it.
    t.select(slot(&t, "tool net"));
    assert_eq!(t.inspect_page(), 0);
    assert!(!t.inspect_name_editing());
    assert_eq!(t.inspect_name_rect(), Vec::<u32>::new());
}

#[test]
fn inspecting_pins_a_two_page_card_that_clicks_turn() {
    let mut t = paused();
    t.select(slot(&t, "creature neon_tetra"));
    down(&mut t, PRIMARY, 30, 20);
    t.pointer_up(PRIMARY);
    assert_eq!((t.inspect_page(), t.inspect_rect()), (0, Vec::new()));
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 30, 20);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 1);
    let rect = t.inspect_rect();
    assert_eq!(rect.len(), 4);
    // Beside the tetra (cell 30 at scale 4), on its right, inside the canvas.
    assert!(rect[0] > 30 * 4 && rect[0] + rect[2] <= 512 && rect[1] + rect[3] <= 288);
    // A click on it turns the page and changes nothing in the tank.
    let hash = t.state_hash();
    let (x, y) = (f64::from(rect[0] + 2), f64::from(rect[1] + 2));
    assert_eq!(t.pointer_down(PRIMARY, x, y), 0);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 2);
    assert_eq!(
        t.take_inspection(),
        "creature neon_tetra 1 water 30 20 adult 180 100 100 male - - Hunger:0:critical,Social:0:critical "
    );
    t.pointer_down(PRIMARY, x, y);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 1);
    assert_eq!(t.state_hash(), hash);
    // Inspecting water moves it to that cell: one page, so a click on it
    // keeps it there.
    down(&mut t, PRIMARY, 100, 20);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 1);
    let rect = t.inspect_rect();
    assert!(
        rect[0] > 100 * 4 || rect[0] + rect[2] < 100 * 4,
        "beside the cell"
    );
    let (x, y) = (f64::from(rect[0] + 2), f64::from(rect[1] + 2));
    t.pointer_down(PRIMARY, x, y);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 1);
    assert_eq!(t.take_inspection(), "cell water 100 20");
    // Inspected again, then the net picked: the card goes, so it can't
    // catch the net's clicks (the Tank tests cover a creature leaving).
    down(&mut t, PRIMARY, 30, 20);
    t.pointer_up(PRIMARY);
    assert_eq!(t.inspect_page(), 1);
    // Another tool removes it; selecting the same tool again doesn't.
    t.select(slot(&t, "tool inspect"));
    assert_eq!(t.inspect_page(), 1);
    t.select(slot(&t, "tool net"));
    assert_eq!(t.inspect_page(), 0);
    down(&mut t, PRIMARY, 30, 20);
    drag(&mut t, 30, 0);
    assert_eq!(t.pointer_up(PRIMARY), 1);
    assert_eq!((t.inspect_page(), t.inspect_rect()), (0, Vec::new()));
    // A device gets a card too.
    t.select(slot(&t, "device internal_filter"));
    down(&mut t, PRIMARY, 4, 30);
    t.pointer_up(PRIMARY);
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 6, 40);
    t.pointer_up(PRIMARY);
    assert!(t.take_inspection().starts_with("device internal_filter "));
    assert_eq!(t.inspect_page(), 1);
    assert_eq!(t.inspect_rect().len(), 4);
}

#[test]
fn device_inspect_rinse_and_guard_round_trip() {
    let mut t = paused();
    t.select(slot(&t, "device internal_filter"));
    down(&mut t, PRIMARY, 4, 30);
    t.pointer_up(PRIMARY);
    t.step();
    let dev = t.tank.world().devices().next().unwrap().id;
    assert!(t.tank.world_mut().set_device_media_load(dev, 0.8));
    assert!(t.tank.world_mut().set_device_maturity(dev, 0.6));
    let _ = t.save();

    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 4, 30);
    t.pointer_up(PRIMARY);
    let line = t.take_inspection();
    assert!(
        line.starts_with("device internal_filter 1 left ") && line.contains(" 80 60 off no "),
        "unexpected device inspect line: {line}"
    );

    let toggled = t.toggle_device_guard(1).unwrap();
    assert!(toggled.contains(" 80 60 on no "), "guard on: {toggled}");
    assert!(t.save_needed());

    let rinsed = t.rinse_device(1, "gentle").unwrap();
    assert!(rinsed.contains(" 0 60 on no "), "gentle rinse: {rinsed}");

    let hard = t.rinse_device(1, "hard").unwrap();
    assert!(hard.contains(" 0 15 on no "), "hard rinse: {hard}");

    let replaced = t.rinse_device(1, "replace").unwrap();
    assert!(replaced.contains(" 0 0 on no "), "replace: {replaced}");

    assert!(t.rinse_device(1, "bogus").is_err());
    assert_eq!(
        t.rinse_device(99, "gentle").unwrap_err(),
        "That device is no longer in the tank"
    );
    assert!(t.toggle_device_guard(99).is_err());

    // DV4: sponge filter mounts on the bed and reports `bed` + `fry_safe = yes`.
    let w = t.tank().world().width();
    let h = t.tank().world().height();
    t.select(slot(&t, "device sponge_filter"));
    let sx = i32::try_from(w / 2).unwrap();
    down(&mut t, PRIMARY, sx, i32::try_from(h / 2).unwrap());
    t.pointer_up(PRIMARY);
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, sx, i32::try_from(h - 6).unwrap());
    t.pointer_up(PRIMARY);
    let sponge_line = t.take_inspection();
    assert!(
        sponge_line.starts_with("device sponge_filter 2 bed ")
            && sponge_line.contains(" 0 0 off yes "),
        "unexpected sponge inspect line: {sponge_line}"
    );

    // DV5: HOB filter mounts on the rim and reports `rim` + `fry_safe = no`.
    t.select(slot(&t, "device hob_filter"));
    let hx = i32::try_from(w / 2).unwrap();
    down(&mut t, PRIMARY, hx, 0);
    t.pointer_up(PRIMARY);
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, hx, 4);
    t.pointer_up(PRIMARY);
    let hob_line = t.take_inspection();
    assert!(
        hob_line.starts_with("device hob_filter 3 rim ") && hob_line.contains(" 0 0 off no "),
        "unexpected HOB inspect line: {hob_line}"
    );

    // DV7: prefab bubbler mounts on the bed and reports `bed air` (no filtration).
    t.select(slot(&t, "device bubbler"));
    let bx = i32::try_from(w / 4).unwrap();
    down(&mut t, PRIMARY, bx, i32::try_from(h / 2).unwrap());
    t.pointer_up(PRIMARY);
    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, bx, i32::try_from(h - 2).unwrap());
    t.pointer_up(PRIMARY);
    let bubbler_line = t.take_inspection();
    assert!(
        bubbler_line.starts_with("device bubbler 4 bed air "),
        "unexpected bubbler inspect line: {bubbler_line}"
    );
    for mode in ["gentle", "hard", "replace"] {
        assert_eq!(
            t.rinse_device(4, mode).unwrap_err(),
            "That device has no filter media"
        );
    }
}

#[test]
fn easy_mode_reaches_the_world() {
    let mut t = paused();
    assert!(!t.easy_mode());
    t.set_easy_mode(true);
    assert!(t.easy_mode() && t.tank.world().easy_mode());
    t.set_easy_mode(false);
    assert!(!t.tank.world().easy_mode());
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
    let pinches = u32::try_from(cheats::FEED_PINCHES).unwrap();
    assert_eq!(t.feed(), pinches);
    assert_eq!(t.material_count(cheats::FEED_FOOD), pinches);
}

#[test]
fn the_konami_code_opens_the_cheat_field_and_cheats_run() {
    let mut t = paused();
    let keys = [
        "ArrowUp",
        "ArrowUp",
        "ArrowDown",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "ArrowLeft",
        "ArrowRight",
        "b",
        "A",
    ];
    let fired: Vec<bool> = keys.iter().map(|k| t.cheat_key(k)).collect();
    assert_eq!(fired.iter().filter(|&&f| f).count(), 1);
    assert!(fired[9]);
    // Any other key starts over.
    for k in &keys[..9] {
        t.cheat_key(k);
    }
    assert!(!t.cheat_key("x"));
    assert!(!t.cheat_key("a"));
    // Shift for a capital B doesn't break the code.
    for k in &keys[..8] {
        t.cheat_key(k);
    }
    assert!(!t.cheat_key("Shift"));
    assert!(!t.cheat_key("B"));
    assert!(t.cheat_key("a"));

    let before = t.creatures();
    assert_eq!(t.cheat("spawn guppy 2").unwrap(), "spawned 2 guppy");
    assert_eq!(t.creatures(), before + 2);
    assert!(t.cheat("feed").unwrap().starts_with("fed"));
    assert!(t.save_needed());
    assert_eq!(
        t.cheat("xyzzy").unwrap_err(),
        "unknown cheat \"xyzzy\"; try help"
    );
}

#[test]
fn field_overlay_round_trips() {
    let mut t = tank();
    assert_eq!(t.field_overlay(), "");
    t.set_field_overlay("co2").unwrap();
    assert_eq!(t.field_overlay(), "co2");
    t.set_field_overlay("flow").unwrap();
    assert_eq!(t.field_overlay(), "flow");
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
fn sway_is_off_by_default_and_toggles() {
    let mut t = tank();
    assert!(!t.sway());
    t.set_sway(true);
    assert!(t.sway());
    t.set_on_battery(true);
    t.set_hidden(true);
    t.set_sway(false);
    assert!(!t.sway());
}

#[test]
fn oled_mode_is_off_by_default_and_toggles() {
    let mut t = tank();
    assert!(!t.oled());
    t.set_oled(true);
    assert!(t.oled());
    // The frame it changes is wanted even without a GPU to draw it.
    assert_ne!(t.frame(0.0) & FRAME_WANTED, 0);
    assert!(!t.save_needed(), "a render setting isn't a tank edit");
    t.set_oled(false);
    assert!(!t.oled());
}

#[test]
fn ruler_is_off_by_default_and_toggles() {
    let mut t = tank();
    assert!(!t.ruler());
    t.set_ruler(true);
    assert!(t.ruler());
    // The frame it changes is wanted even without a GPU to draw it.
    assert_ne!(t.frame(0.0) & FRAME_WANTED, 0);
    assert!(!t.save_needed(), "a render setting isn't a tank edit");
    t.set_ruler(false);
    assert!(!t.ruler());
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

#[test]
fn fish_avoid_only_an_enabled_pointer_over_the_canvas() {
    let simulate = |enabled, leave, x| {
        let mut t = paused();
        t.select(slot(&t, "creature neon_tetra"));
        down(&mut t, PRIMARY, 64, 40);
        t.pointer_up(PRIMARY);
        drag(&mut t, x, 40);
        // Toggling the setting applies to an already stationary pointer.
        t.set_cursor_avoidance(enabled);
        if leave {
            t.pointer_leave();
        }
        for _ in 0..24 {
            t.step();
        }
        t.tank.world().creatures()[0].cell()
    };
    assert_eq!(simulate(false, false, 61), simulate(true, true, 61));
    assert!(simulate(true, false, 61).0 > 70);
    assert!(simulate(true, false, 67).0 < 58);
}

/// Saves (WB3): the native app's format, in both directions.
mod saves {
    use super::*;
    use std::path::PathBuf;

    fn file(path: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
    }

    /// The native T8 fixture (`desktank-sim`'s `busy` scene, seed 1, 1200
    /// ticks) and its state hash; `tests/web/saves.spec.ts` imports the
    /// same file on the web and checks the same hash.
    const NATIVE_FIXTURE: &str = "../desktank-sim/tests/fixtures/v1.dtank";
    const NATIVE_HASH: &str = "54685e4c472e75d9";
    /// The native fixture as this build re-saves it natively. A save carries
    /// the build's material and species name tables, so it is not the v1
    /// file's bytes once ids are added; the web test compares its export of
    /// the imported fixture against this file.
    const NATIVE_RESAVED: &str = "tests/fixtures/v1-resaved.dtank";

    /// The `layers` render scene at seed 1, saved by this crate; the web test
    /// exports the same scene from the page and compares the bytes, so the
    /// wasm build writes exactly what the native one does.
    const SCENE: &str = "../desktank/tests/scenes/layers.toml";
    const SCENE_FIXTURE: &str = "tests/fixtures/layers-seed1.dtank";

    #[test]
    fn a_saved_tank_reopens_where_it_was() {
        let mut t = tank();
        assert!(!t.save_needed(), "a new tank");
        t.select(slot(&t, "material sand"));
        down(&mut t, PRIMARY, 64, 20);
        t.pointer_up(PRIMARY);
        assert!(t.save_needed());
        for _ in 0..40 {
            t.step();
        }
        let bytes = t.save();
        assert!(!t.save_needed(), "saved");
        // A different canvas: the grid keeps its size, letterboxed.
        let mut u = WebTank::from_save(&bytes, 1000, 300).unwrap();
        assert_eq!((u.width(), u.height()), (128, 72));
        assert_eq!(u.cell_scale(), 4);
        assert_eq!(u.state_hash(), t.state_hash());
        assert!(!u.save_needed(), "a loaded tank");
        assert_eq!(u.save(), bytes, "re-encodes identically");
        // Both carry on tick for tick.
        for _ in 0..40 {
            t.step();
            u.step();
        }
        assert_eq!(u.state_hash(), t.state_hash());
        assert!(t.save_needed());
    }

    #[test]
    fn ticks_and_paused_edits_need_a_save() {
        let mut t = tank();
        t.frame(0.0);
        let _ = t.save();
        t.frame(1000.0 * MS);
        assert!(t.save_needed(), "ticks ran");
        let mut p = paused();
        p.feed();
        assert!(p.save_needed(), "feeding while paused");
        let _ = p.save();
        p.frame(5000.0 * MS);
        assert!(!p.save_needed(), "paused: nothing ran");
        p.select(slot(&p, "creature neon_tetra"));
        down(&mut p, PRIMARY, 64, 40);
        assert!(p.save_needed(), "a creature added");
    }

    #[test]
    fn catch_up_matches_the_world_and_survives_a_save() {
        let mut t = paused();
        let mut expected = t.tank().world().clone();
        let report = expected.catch_up(87);

        assert_eq!(
            t.catch_up(87),
            [
                87,
                count(report.births),
                count(report.old_age_deaths + report.starvation_deaths + report.poor_water_deaths),
            ]
        );
        assert!(t.save_needed());
        let bytes = t.save();
        assert!(!t.save_needed());
        assert_eq!(bytes, save::encode(&expected));
        let loaded = WebTank::from_save(&bytes, 512, 288).unwrap();
        assert_eq!(loaded.state_hash(), t.state_hash());
    }

    #[test]
    fn catch_up_skips_short_gaps_and_caps_long_ones() {
        let mut t = paused();
        let hash = t.state_hash();
        assert_eq!(t.catch_up(0), [0, 0, 0]);
        assert_eq!(t.catch_up(59), [0, 0, 0]);
        assert_eq!(t.state_hash(), hash);
        assert!(!t.save_needed());

        let seconds = desktank_sim::MAX_CATCH_UP_SECONDS;
        assert_eq!(t.catch_up(u32::MAX)[0], seconds);
        assert_eq!(
            t.tank().world().tick_count(),
            u64::from(seconds * desktank_sim::TICKS_PER_SECOND)
        );
        assert!(t.save_needed());
    }

    #[test]
    fn rejects_what_isnt_a_save() {
        assert!(WebTank::from_save(b"not a save", 100, 100).is_err());
        let mut bytes = tank().save();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x40;
        assert!(WebTank::from_save(&bytes, 100, 100).is_err());
        // Too large for WebGL2.
        let big = World::new(
            WorldConfig {
                width: 2100,
                height: 8,
                seed: 1,
                water_level: 0.5,
            },
            builtin().unwrap(),
        )
        .unwrap();
        let err = WebTank::from_save(&save::encode(&big), 100, 100)
            .err()
            .unwrap();
        assert!(err.contains("2100x8"), "{err}");
    }

    #[test]
    fn a_native_save_opens() {
        let bytes = std::fs::read(file(NATIVE_FIXTURE)).unwrap();
        let mut t = WebTank::from_save(&bytes, 1280, 720).unwrap();
        assert_eq!((t.width(), t.height(), t.tick_count()), (200, 120, 1200.0));
        assert_eq!(t.cell_scale(), 6, "letterboxed at the largest even fit");
        assert_eq!(t.state_hash(), NATIVE_HASH);
        let native = save::encode(&save::decode(&bytes, builtin().unwrap()).unwrap());
        let exported = t.save();
        assert!(exported == native, "exports what native re-saves");
        let path = file(NATIVE_RESAVED);
        if std::env::var_os("DESKTANK_BLESS").is_some() {
            std::fs::write(&path, &native).unwrap();
        }
        assert!(
            std::fs::read(&path).unwrap() == native,
            "re-bless with DESKTANK_BLESS=1"
        );
    }

    #[test]
    fn the_scene_fixture_is_what_native_writes() {
        let source = std::fs::read_to_string(file(SCENE)).unwrap();
        let mut t = WebTank::from_script(&source, 1, 1280, 720).unwrap();
        let bytes = t.save();
        let path = file(SCENE_FIXTURE);
        if std::env::var_os("DESKTANK_BLESS").is_some() {
            std::fs::write(&path, &bytes).unwrap();
        }
        assert!(
            std::fs::read(&path).unwrap() == bytes,
            "re-bless with DESKTANK_BLESS=1"
        );
    }
}

#[test]
fn creature_cap_clamps_marks_dirty_and_persists_in_saves() {
    let mut t = paused();
    assert_eq!(t.default_creature_cap(), 60);
    assert_eq!(t.max_creature_cap(), 128);
    assert_eq!(t.creature_cap(), 60);
    assert_eq!(t.stock_count(), 0);
    assert!(!t.save_needed());

    // Setting the same cap doesn't mark dirty.
    assert_eq!(t.set_creature_cap(60), 60);
    assert!(!t.save_needed());

    // Changing the cap marks dirty and clamps to 1..=max.
    assert_eq!(t.set_creature_cap(0), 1);
    assert!(t.save_needed());
    assert_eq!(t.set_creature_cap(999), t.max_creature_cap());
    assert_eq!(t.creature_cap(), 128);

    // Stocked creatures raise the lower bound; live food does not.
    t.select(slot(&t, "creature neon_tetra"));
    for x in [30, 60, 90] {
        down(&mut t, PRIMARY, x, 20);
        t.pointer_up(PRIMARY);
    }
    assert_eq!(t.stock_count(), 3);
    t.select(slot(&t, "live_food live_bloodworm"));
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    assert!(t.creatures() > t.stock_count());

    assert_eq!(t.set_creature_cap(1), 3);
    assert_eq!(t.creature_cap(), 3);

    // Round-trip through save preserves the cap.
    let bytes = t.save();
    assert!(!t.save_needed());
    let loaded = WebTank::from_save(&bytes, 512, 288).unwrap();
    assert_eq!(loaded.creature_cap(), 3);
    assert_eq!(loaded.stock_count(), 3);
}

#[test]
fn refused_creature_add_reports_cap_live_cap_and_no_room() {
    let mut t = paused();
    t.select(slot(&t, "creature neon_tetra"));
    assert_eq!(t.take_refusal(), "");

    // Deep inside a solid block reports no_room and doesn't mark dirty.
    let sand = t.tank().world().materials().id("sand").unwrap();
    t.tank
        .world_mut()
        .fill_rect(
            desktank_sim::Rect {
                x0: 0,
                y0: 45,
                x1: 128,
                y1: 72,
            },
            sand,
            desktank_sim::Brush::Replace,
        )
        .unwrap();
    let _ = t.save();
    down(&mut t, PRIMARY, 64, 65);
    t.pointer_up(PRIMARY);
    assert_eq!(t.take_refusal(), "no_room neon_tetra");
    assert_eq!(t.take_refusal(), "", "taken once");
    assert!(!t.save_needed());

    // Filling to cap reports cap <stock> <cap> <species>.
    assert_eq!(t.set_creature_cap(2), 2);
    for x in [40, 80] {
        down(&mut t, PRIMARY, x, 40);
        t.pointer_up(PRIMARY);
        assert_eq!(t.take_refusal(), "");
    }
    let _ = t.save();
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(t.take_refusal(), "cap 2 2 neon_tetra");
    assert!(!t.save_needed(), "refused add leaves tank clean");

    // Raising the cap allows adding immediately.
    assert_eq!(t.set_creature_cap(3), 3);
    down(&mut t, PRIMARY, 60, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(t.take_refusal(), "");
    assert_eq!(t.stock_count(), 3);

    // Live food reports live_cap once MAX_LIVE_FOOD is reached.
    t.select(slot(&t, "live_food live_bloodworm"));
    while t.tank().world().live_food_count() < desktank_sim::MAX_LIVE_FOOD {
        down(&mut t, PRIMARY, 64, 40);
        t.pointer_up(PRIMARY);
        assert_eq!(t.take_refusal(), "");
    }
    down(&mut t, PRIMARY, 64, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(t.take_refusal(), "live_cap 32 32 live_bloodworm");
}

#[test]
fn magnify_tool_controls_lens_without_dirtying_world() {
    let mut t = paused();
    assert_eq!(zoom_levels(), [2, 4, 6, 8]);
    assert!(!t.is_magnify());
    assert_eq!(t.zoom(), 2);
    assert_eq!(t.tank().magnifier(), None);

    t.select(slot(&t, "tool magnify"));
    assert!(t.is_magnify());
    let _ = t.save();

    // Moving the pointer places the magnifier without marking the tank dirty.
    drag(&mut t, 60, 35);
    assert_eq!(
        t.tank().magnifier(),
        Some(Magnifier {
            cell: (60, 35),
            zoom: 2,
        })
    );
    assert!(!t.save_needed());

    // Clicking cycles magnification without dirtying the save.
    down(&mut t, PRIMARY, 60, 35);
    t.pointer_up(PRIMARY);
    assert_eq!(t.zoom(), 4);
    assert_eq!(
        t.tank().magnifier(),
        Some(Magnifier {
            cell: (60, 35),
            zoom: 4,
        })
    );
    assert!(!t.save_needed());

    // Stepping and setting zoom update the lens.
    assert_eq!(t.step_zoom(1), 6);
    assert_eq!(t.tank().magnifier().unwrap().zoom, 6);
    assert_eq!(t.set_zoom(8), 8);
    assert_eq!(t.tank().magnifier().unwrap().zoom, 8);
    assert_eq!(t.step_zoom(5), 8);
    assert_eq!(t.step_zoom(-10), 2);
    assert_eq!(t.tank().magnifier().unwrap().zoom, 2);

    // Dragging pans without cycling zoom.
    down(&mut t, PRIMARY, 60, 35);
    drag(&mut t, 70, 40);
    t.pointer_up(PRIMARY);
    assert_eq!(t.zoom(), 2);
    assert_eq!(t.tank().magnifier().unwrap().cell, (70, 40));

    // Leaving hides the lens.
    t.pointer_leave();
    assert_eq!(t.tank().magnifier(), None);
}

#[test]
fn a_whole_plant_start_shows_a_flippable_ghost_and_places_one_plant() {
    let mut t = paused();
    t.select(slot(&t, "plant hygrophila"));
    // A propagule has no ghost and nothing to flip.
    assert_eq!(t.plant_propagule(), "cuttings");
    assert!(!t.can_flip_device());
    drag(&mut t, 60, 58);
    assert!(t.tools.plant_ghost(t.tank.world()).is_none());
    assert!(!t.flip_device(), "a cutting doesn't flip");
    // A mature plant does, and its ghost follows the pointer.
    assert!(t.set_plant_propagule("mature"));
    assert!(t.can_flip_device());
    let ghost = t.tools.plant_ghost(t.tank.world()).unwrap();
    assert!(!ghost.fits, "no substrate to root in yet");
    let sand = t.tank.world().materials().id("sand").unwrap();
    let bed = desktank_sim::Rect {
        x0: 0,
        y0: 62,
        x1: 128,
        y1: 72,
    };
    t.tank
        .world_mut()
        .fill_rect(bed, sand, desktank_sim::Brush::Replace)
        .unwrap();
    t.tank.cells_edited();
    drag(&mut t, 60, 58);
    let ghost = t.tools.plant_ghost(t.tank.world()).unwrap();
    assert!(ghost.fits && ghost.cells.len() > 3);
    assert!(t.flip_device());
    let mirrored = t.tools.plant_ghost(t.tank.world()).unwrap();
    assert_ne!(mirrored.cells, ghost.cells, "flipped is the mirror image");
    assert!(!t.flip_device());
    // The click puts it down whole, in one step.
    assert_eq!(t.plant_count(), 0);
    down(&mut t, PRIMARY, 60, 58);
    t.pointer_up(PRIMARY);
    assert_eq!(t.plant_count(), 1);
    // A juvenile is smaller than a mature plant of the same species.
    assert_eq!(t.next_plant_propagule(), "seeds", "the picks wrap");
    assert!(t.set_plant_propagule("juvenile"));
    let young = t.tools.plant_ghost(t.tank.world()).unwrap();
    assert!(young.cells.len() < ghost.cells.len());
    // Leaving the tank clears it.
    t.pointer_leave();
    assert!(t.tools.plant_ghost(t.tank.world()).is_none());
}

#[test]
fn under_gravel_filter_placement_flips_the_tube_and_inspects_as_fry_safe() {
    let mut t = paused();
    t.select(slot(&t, "device under_gravel_filter"));
    assert!(t.can_flip_device());
    assert!(!t.device_flipped());
    down(&mut t, PRIMARY, 32, 36);
    t.pointer_up(PRIMARY);
    assert!(t.flip_device());
    down(&mut t, PRIMARY, 96, 36);
    t.pointer_up(PRIMARY);

    // At this height the upright tube is on one side of the footprint.
    let world = t.tank().world();
    assert_eq!(world.devices().count(), 2);
    assert_eq!(world.device_at(20, 54), Some(DeviceId(1)));
    assert_eq!(world.device_at(43, 54), None);
    assert_eq!(world.device_at(84, 54), None);
    assert_eq!(world.device_at(107, 54), Some(DeviceId(2)));

    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 107, 54);
    t.pointer_up(PRIMARY);
    let line = t.take_inspection();
    assert!(
        line.starts_with("device under_gravel_filter 2 floor ")
            && line.ends_with(" off yes 107 54"),
        "unexpected under-gravel filter inspect line: {line}"
    );
}

#[test]
fn under_gravel_panel_inspects_connection_to_the_main_filter() {
    let mut t = paused();
    t.select(slot(&t, "device under_gravel_panel"));
    down(&mut t, PRIMARY, 32, 71);
    t.pointer_up(PRIMARY);

    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 32, 71);
    t.pointer_up(PRIMARY);
    assert_eq!(
        t.take_inspection(),
        "device under_gravel_panel 1 floor disconnected 32 71"
    );

    t.select(slot(&t, "device under_gravel_filter"));
    down(&mut t, PRIMARY, 56, 71);
    t.pointer_up(PRIMARY);

    t.select(slot(&t, "tool inspect"));
    down(&mut t, PRIMARY, 32, 71);
    t.pointer_up(PRIMARY);
    assert_eq!(
        t.take_inspection(),
        "device under_gravel_panel 1 floor connected 32 71"
    );
}

#[test]
fn flip_device_only_applies_to_flippable_device_tools() {
    let mut t = paused();
    // Default tool (sand) and non-flippable tools/devices cannot flip.
    assert!(!t.can_flip_device());
    assert!(!t.device_flipped());
    assert!(!t.flip_device());
    assert!(!t.device_flipped());

    for entry in [
        "tool scoop",
        "tool inspect",
        "tool magnify",
        "device bubbler",
        "device sponge_filter",
        "device under_gravel_panel",
        "plant java_moss",
        "creature neon_tetra",
    ] {
        t.select(slot(&t, entry));
        assert!(!t.can_flip_device(), "{entry} should not be flippable");
        assert!(!t.flip_device(), "{entry} should ignore flip_device");
        assert!(!t.device_flipped());
    }

    // HOB filter (rim mount) also supports flipping.
    t.select(slot(&t, "device hob_filter"));
    assert!(t.can_flip_device());
    assert!(t.flip_device());
    assert!(t.device_flipped());
    assert!(!t.flip_device());
    assert!(!t.device_flipped());

    // Internal filter supports flipping.
    t.select(slot(&t, "device internal_filter"));
    assert!(t.can_flip_device());
    assert!(t.flip_device());
    assert!(t.device_flipped());

    // Switching away preserves the stored setting without letting flip_device toggle it.
    t.select(slot(&t, "material sand"));
    assert!(!t.can_flip_device());
    assert!(t.flip_device());
    assert!(t.device_flipped());

    // Switching back restores flippability and can toggle off again.
    t.select(slot(&t, "device internal_filter"));
    assert!(t.can_flip_device());
    assert!(!t.flip_device());
    assert!(!t.device_flipped());
}

#[test]
fn the_tour_starts_and_ends_and_a_resize_keeps_it() {
    let mut t = tank();
    assert!(!t.touring());
    t.set_tour(true, true, 3);
    assert!(t.touring());
    assert_eq!(t.tank().tour().map(|s| (s.seed, s.card)), Some((3, true)));
    // The canvas size reaches the tour, so a resize doesn't end it.
    t.resize(640, 360);
    assert!(t.touring());
    t.set_tour(false, false, 0);
    assert!(!t.touring());
    assert!(t.tank().camera().is_none());
}
