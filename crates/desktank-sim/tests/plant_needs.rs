//! Plant care readings (#443): how well the light, CO₂ and nutrients around
//! an individual plant suit its species, read from the world as it is.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use desktank_sim::{
    Brush, Factor, FieldKind, Limit, MaterialTable, Need, NeedLevel, NeedReading, PlantId,
    PlantSpeciesId, Propagule, Rect, World, WorldConfig,
};

const MINUTE: u32 = 1200;

/// A 160-wide tank with an aquasoil bed 6 rows deep.
fn tank(height: usize, seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: 160,
            height,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let soil = w.materials().id("aquasoil").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: height - 6,
            x1: 160,
            y1: height,
        },
        soil,
        Brush::Replace,
    )
    .unwrap();
    w
}

fn species(w: &World, name: &str) -> PlantSpeciesId {
    w.plant_species().id(name).unwrap()
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// A rock lid over columns `x0..x1`, `rows` thick, glued in place.
fn lid(w: &mut World, x0: usize, x1: usize, rows: usize) {
    let rock = w.materials().id("rock").unwrap();
    w.fill_rect(
        Rect {
            x0,
            y0: 8,
            x1,
            y1: 8 + rows,
        },
        rock,
        Brush::Replace,
    )
    .unwrap();
    let body = w.body_at(x0, 8).unwrap();
    assert!(w.set_anchored(body, true));
}

/// The readings of the only plant of `s`.
fn needs(w: &World, id: PlantId) -> [NeedReading; 3] {
    w.plant_needs(id).unwrap()
}

fn only(w: &World, s: PlantSpeciesId) -> PlantId {
    let found: Vec<_> = w.plants().filter(|p| p.species == s).collect();
    assert_eq!(found.len(), 1, "one plant of the species");
    found[0].id
}

fn satisfaction(readings: [NeedReading; 3], need: Need) -> u8 {
    readings
        .iter()
        .find(|r| r.need == need)
        .unwrap()
        .satisfaction
}

#[test]
fn every_plant_has_light_co2_and_nutrient_readings_in_that_order() {
    let mut w = tank(64, 3);
    let hyg = species(&w, "hygrophila");
    w.paint_plant(80, 30, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 2 * MINUTE);
    let id = only(&w, hyg);
    let readings = needs(&w, id);
    assert_eq!(
        readings.map(|r| r.need),
        [Need::Light, Need::Co2, Need::Nutrients]
    );
    assert_eq!(
        readings.map(|r| r.need.label()),
        ["Light", "CO₂", "Nutrients"]
    );
    assert!(readings.iter().all(|r| r.satisfaction <= 100));
    assert!(readings[0].describe().starts_with("Light "));
    assert!(w.plant_needs(PlantId(9999)).is_none(), "no such plant");
}

#[test]
fn the_reading_the_limit_names_is_the_lowest() {
    // Rotala with no CO₂ injection is held back most by CO₂ (the
    // inspect card's Growth: line), so its CO₂ meter is the shortest.
    let mut w = tank(64, 6);
    let rotala = species(&w, "rotala");
    w.paint_plant(80, 30, 0, rotala, Propagule::Cuttings)
        .unwrap();
    run(&mut w, 4 * MINUTE);
    let p = w.plants().find(|p| p.species == rotala).unwrap();
    assert_eq!(p.limit, Limit::TooLow(Factor::Co2));
    let readings = needs(&w, p.id);
    let lowest = readings.iter().min_by_key(|r| r.satisfaction).unwrap();
    assert_eq!(lowest.need, Need::Co2, "{readings:?}");
    assert!(lowest.satisfaction < 80, "{readings:?}");
}

#[test]
fn dimming_the_light_lowers_the_light_reading_only() {
    let grow = |rows: usize| {
        let mut w = tank(48, 1);
        if rows > 0 {
            lid(&mut w, 0, 160, rows);
        }
        let cabomba = species(&w, "cabomba");
        w.paint_plant(80, 30, 0, cabomba, Propagule::Cuttings)
            .unwrap();
        run(&mut w, 4 * MINUTE);
        needs(&w, only(&w, cabomba))
    };
    let (open, shaded) = (grow(0), grow(5));
    let (open_light, shaded_light) = (
        satisfaction(open, Need::Light),
        satisfaction(shaded, Need::Light),
    );
    assert!(
        open_light >= shaded_light + 25,
        "open {open:?}, shaded {shaded:?}"
    );
    assert!(shaded_light < 50, "{shaded:?}");
    // Light does not move the other factors (the plants differ in size).
    let near = |a: u8, b: u8| a.abs_diff(b) <= 20;
    assert!(near(
        satisfaction(open, Need::Nutrients),
        satisfaction(shaded, Need::Nutrients)
    ));
}

#[test]
fn changing_co2_and_nutrients_moves_their_readings_at_once() {
    let mut w = tank(64, 2);
    let hyg = species(&w, "hygrophila");
    w.paint_plant(80, 30, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 3 * MINUTE);
    let id = only(&w, hyg);
    let (x, y) = w.plant(id).unwrap().base;
    let spread = |w: &mut World, kind: FieldKind, amount: f32| {
        for dy in -12_i64..=2 {
            for dx in -8_i64..=8 {
                let (cx, cy) = (x as i64 + dx, y as i64 + dy);
                if cx >= 0 && cy >= 0 {
                    w.add_to_field(kind, cx as usize, cy as usize, amount);
                }
            }
        }
    };
    let start = needs(&w, id);
    let level = |r: [NeedReading; 3], need| satisfaction(r, need);
    assert!(level(start, Need::Co2) < 75, "no injection: {start:?}");
    // Dosing CO₂ brings it to the optimum, with no tick in between: the
    // reading follows the field, not the plant's five-second samples.
    spread(&mut w, FieldKind::Co2, 0.012);
    let dosed = needs(&w, id);
    assert!(
        level(dosed, Need::Co2) > level(start, Need::Co2) + 10,
        "{start:?} -> {dosed:?}"
    );
    // Too much is no better than too little: far over the range is 0.
    spread(&mut w, FieldKind::Co2, 100.0);
    assert_eq!(level(needs(&w, id), Need::Co2), 0);
    // Stripping the nutrients falls to 0 at once; the light is unmoved.
    let fed = level(start, Need::Nutrients);
    spread(&mut w, FieldKind::Nutrient, -50.0);
    let starved = needs(&w, id);
    assert!(
        level(starved, Need::Nutrients) < fed,
        "{start:?} -> {starved:?}"
    );
    assert_eq!(level(starved, Need::Light), level(start, Need::Light));
    assert_eq!(starved[2].level(), NeedLevel::Critical);
}

#[test]
fn two_plants_read_their_own_conditions() {
    let mut w = tank(48, 4);
    let cabomba = species(&w, "cabomba");
    w.paint_plant(40, 30, 0, cabomba, Propagule::Cuttings)
        .unwrap();
    w.paint_plant(120, 30, 0, cabomba, Propagule::Cuttings)
        .unwrap();
    lid(&mut w, 0, 80, 5);
    run(&mut w, 3 * MINUTE);
    let mut plants: Vec<_> = w.plants().collect();
    plants.sort_by_key(|p| p.base.0);
    let [shaded, lit] = plants[..] else {
        panic!("two plants")
    };
    let (a, b) = (needs(&w, shaded.id), needs(&w, lit.id));
    assert_ne!(a, b);
    assert!(
        satisfaction(a, Need::Light) + 20 < satisfaction(b, Need::Light),
        "shaded {a:?}, lit {b:?}"
    );
}

#[test]
fn readings_are_a_pure_function_of_the_world() {
    let build = || {
        let mut w = tank(64, 5);
        let hyg = species(&w, "hygrophila");
        w.paint_plant(80, 30, 0, hyg, Propagule::Cuttings).unwrap();
        run(&mut w, 2 * MINUTE);
        w
    };
    let (a, b) = (build(), build());
    let id = a.plants().next().unwrap().id;
    assert_eq!(needs(&a, id), needs(&b, id));
    // Asking changes nothing: same readings twice, and a world that goes
    // on exactly as one that was never asked.
    let (mut asked, mut unasked) = (build(), build());
    assert_eq!(needs(&asked, id), needs(&asked, id));
    run(&mut asked, 200);
    run(&mut unasked, 200);
    assert_eq!(needs(&asked, id), needs(&unasked, id));
    assert_eq!(asked.plant(id), unasked.plant(id));
}
