//! Resting at night (SPEC §2.4, #195): creatures settle at their bedtime
//! where their species rests (the bed, among plants, under the surface),
//! hold still, and wake at dawn, when fed or when disturbed; nocturnal
//! species and live food stay up; and rest is deterministic and saved.
#![allow(clippy::cast_possible_wrap, clippy::cast_precision_loss)]

use desktank_sim::night::{BEDTIME, STAGGER_MINUTES, WAKE_TIME, WOKEN_TICKS};
use desktank_sim::{
    Brush, CreatureId, MaterialTable, Rect, TICKS_PER_SECOND, World, WorldConfig, cheats, save,
};

const WIDTH: usize = 200;
const HEIGHT: usize = 80;
const BED: usize = 72;
/// Late enough that every creature is past its own bedtime.
const MIDNIGHT: u16 = 0;
const LATE: u16 = BEDTIME + STAGGER_MINUTES;
/// Late enough that every creature is past its own wake time.
const MORNING: u16 = WAKE_TIME + STAGGER_MINUTES;
const NOON: u16 = 12 * 60;

fn world(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let sand = w.materials().id("sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: BED,
            x1: WIDTH,
            y1: HEIGHT,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    w.set_easy_mode(true);
    w
}

fn add(w: &mut World, name: &str, x: i64, y: i64) -> CreatureId {
    let s = w.species().id(name).unwrap();
    w.add_creature(s, x, y).unwrap()
}

fn run(w: &mut World, seconds: u32) {
    for _ in 0..seconds * TICKS_PER_SECOND {
        w.tick();
    }
}

fn asleep(w: &World, id: CreatureId) -> bool {
    w.creature(id).unwrap().asleep()
}

/// A community tank: fish of every zone, a bottom walker and a crawler.
fn community(seed: u64) -> (World, Vec<CreatureId>) {
    let mut w = world(seed);
    let ids = vec![
        add(&mut w, "neon_tetra", 40, 30),
        add(&mut w, "neon_tetra", 50, 32),
        add(&mut w, "betta", 100, 12),
        add(&mut w, "common_goldfish", 150, 30),
        add(&mut w, "corydoras", 80, 68),
        add(&mut w, "nerite", 120, 68),
    ];
    (w, ids)
}

#[test]
fn creatures_fall_asleep_at_night_hold_still_and_wake_at_dawn() {
    let (mut w, ids) = community(7);
    w.set_time_of_day(NOON);
    run(&mut w, 10);
    assert!(ids.iter().all(|&id| !asleep(&w, id)), "awake by day");

    w.set_time_of_day(LATE);
    w.tick();
    assert!(ids.iter().all(|&id| asleep(&w, id)), "asleep past bedtime");
    // A minute to drift to their spots, then nothing moves: no redraws.
    run(&mut w, 60);
    let version = w.creatures_version();
    let at: Vec<_> = ids
        .iter()
        .map(|&id| w.creature(id).unwrap().centre())
        .collect();
    run(&mut w, 30);
    assert_eq!(
        w.creatures_version(),
        version,
        "settled sleepers don't redraw"
    );
    let now: Vec<_> = ids
        .iter()
        .map(|&id| w.creature(id).unwrap().centre())
        .collect();
    assert_eq!(now, at);

    w.set_time_of_day(MORNING);
    w.tick();
    assert!(ids.iter().all(|&id| !asleep(&w, id)), "awake at dawn");
    run(&mut w, 5);
    assert_ne!(w.creatures_version(), version, "they swim off again");
}

#[test]
fn bedtimes_are_staggered() {
    let mut w = world(3);
    let ids: Vec<_> = (0..12)
        .map(|i| add(&mut w, "harlequin_rasbora", 20 + 14 * i, 30))
        .collect();
    w.set_time_of_day(BEDTIME + STAGGER_MINUTES / 2);
    w.tick();
    let sleeping = ids.iter().filter(|&&id| asleep(&w, id)).count();
    assert!(
        (1..ids.len()).contains(&sleeping),
        "{sleeping} of {} asleep half an hour in",
        ids.len()
    );
}

#[test]
fn each_species_rests_where_it_naturally_would() {
    let mut w = world(11);
    let surface = w.surface_row();
    // A stand of plants in a patch of soil around x = 150.
    let soil = w.materials().id("aquasoil").unwrap();
    let patch = Rect {
        x0: 130,
        y0: BED,
        x1: 172,
        y1: HEIGHT,
    };
    w.fill_rect(patch, soil, Brush::Replace).unwrap();
    let kind = w.plant_species().id("hygrophila").unwrap();
    for x in [138, 150, 162] {
        let from = w.plant_species().get(kind).unwrap().default_propagule();
        w.paint_plant(x, BED as i64 - 1, 0, kind, from).unwrap();
    }
    run(&mut w, 6 * 60);
    let plant_cells = |w: &World| {
        (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
            .filter(|&(x, y)| w.plant_band(x, y).is_some() && y < BED)
            .collect::<Vec<_>>()
    };
    assert!(!plant_cells(&w).is_empty(), "the plant grew");
    let betta = add(&mut w, "betta", 60, 40);
    let goldfish = add(&mut w, "common_goldfish", 30, 20);
    let tetra = add(&mut w, "neon_tetra", 120, 30);
    w.set_time_of_day(MIDNIGHT);
    run(&mut w, 90);
    let body = |id| w.creature_body(w.creature(id).unwrap());
    assert!(
        body(betta).y0 <= surface + 3,
        "the betta hangs under the surface"
    );
    assert!(
        body(goldfish).y1 + 3 >= BED,
        "the goldfish settles on the bed"
    );
    let (tx, ty) = w.creature(tetra).unwrap().centre();
    let near = plant_cells(&w)
        .iter()
        .any(|&(x, y)| (x as f32 - tx).abs() <= 6.0 && (y as f32 - ty).abs() <= 6.0);
    assert!(near, "the tetra rests among the plants at ({tx}, {ty})");
}

#[test]
fn nocturnal_species_and_live_food_stay_up() {
    let mut w = world(5);
    let loach = add(&mut w, "kuhli_loach", 60, 68);
    let pleco = add(&mut w, "bristlenose_pleco", 120, 68);
    w.set_time_of_day(MIDNIGHT);
    run(&mut w, 5);
    assert!(!asleep(&w, loach) && !asleep(&w, pleco));
}

#[test]
fn feeding_wakes_the_tank_for_a_while() {
    let (mut w, ids) = community(9);
    w.set_time_of_day(MIDNIGHT);
    run(&mut w, 5);
    assert!(ids.iter().all(|&id| asleep(&w, id)));
    assert!(cheats::feed(&mut w) > 0);
    let tetra = ids[0];
    assert!(!asleep(&w, tetra), "flakes wake a flake eater");
    run(&mut w, 1);
    assert!(!asleep(&w, tetra), "and it stays up to eat");
    run(&mut w, u32::from(WOKEN_TICKS) / TICKS_PER_SECOND + 1);
    assert!(asleep(&w, tetra), "then settles again");
}

#[test]
fn the_cursor_and_the_net_wake_a_sleeper() {
    let mut w = world(13);
    let tetra = add(&mut w, "neon_tetra", 60, 30);
    let other = add(&mut w, "neon_tetra", 160, 30);
    w.set_time_of_day(MIDNIGHT);
    run(&mut w, 30);
    assert!(asleep(&w, tetra));
    let (x, y) = w.creature(tetra).unwrap().cell();
    w.set_creature_cursor(Some((x + 2, y)));
    w.tick();
    assert!(!asleep(&w, tetra), "the cursor wakes it");
    assert!(asleep(&w, other), "far off, another sleeps on");
    w.set_creature_cursor(None);

    assert!(w.hold_creature(other));
    assert!(!asleep(&w, other), "the net wakes it");
    assert!(w.creature(other).unwrap().woken() > 0);
}

#[test]
fn a_sleeper_burns_less_energy() {
    let mut w = world(17);
    let awake = add(&mut w, "neon_tetra", 40, 30);
    let sleeper = add(&mut w, "neon_tetra", 140, 30);
    w.set_creature_energy(awake, 100_000);
    w.set_creature_energy(sleeper, 100_000);
    let ticks = 400;
    // Keep one awake with the net's wake-up, renewed each tick.
    w.set_time_of_day(MIDNIGHT);
    for _ in 0..ticks {
        w.hold_creature(awake);
        w.tick();
    }
    let used = |id| 100_000 - w.creature(id).unwrap().energy();
    assert_eq!(used(awake), ticks);
    assert!(used(sleeper) <= ticks / 4 + 1, "{}", used(sleeper));
}

#[test]
fn rest_is_deterministic_and_survives_a_save() {
    let night = |seed| {
        let (mut w, _) = community(seed);
        w.set_time_of_day(BEDTIME + 20);
        run(&mut w, 30);
        w
    };
    let mut a = night(21);
    assert_eq!(a.state_hash(), night(21).state_hash());
    assert!(a.creatures().iter().any(desktank_sim::Creature::asleep));
    let mut b = save::decode(&save::encode(&a), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(b.state_hash(), a.state_hash());
    b.set_time_of_day(a.time_of_day());
    for minute in [BEDTIME + 40, LATE, MIDNIGHT] {
        a.set_time_of_day(minute);
        b.set_time_of_day(minute);
        run(&mut a, 20);
        run(&mut b, 20);
        assert_eq!(a.state_hash(), b.state_hash(), "{minute}");
    }
}

#[test]
fn a_day_tank_hashes_as_if_there_were_no_night() {
    // By day, rest never sets anything: two tanks, one told it is noon,
    // the other never told, stay identical.
    let (mut a, _) = community(23);
    let (mut b, _) = community(23);
    a.set_time_of_day(NOON);
    run(&mut a, 30);
    run(&mut b, 30);
    assert_eq!(a.state_hash(), b.state_hash());
    assert!(a.creatures().iter().all(|c| !c.asleep() && c.woken() == 0));
}
