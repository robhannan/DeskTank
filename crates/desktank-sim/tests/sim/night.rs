//! Resting at night (SPEC §2.4, #195): creatures settle at their bedtime
//! where their species rests (the bed, among plants, under the surface),
//! hold still, and wake at dawn, when fed or when disturbed; nocturnal
//! species and live food stay up; and rest is deterministic and saved.
#![allow(clippy::cast_possible_wrap, clippy::cast_precision_loss)]

use desktank_sim::night::{BEDTIME, STAGGER_MINUTES, WAKE_TIME, WOKEN_TICKS};
use desktank_sim::{
    Brush, CreatureId, DaylightSchedule, MaterialTable, PolarDaylight, Rect, TICKS_PER_SECOND,
    World, WorldConfig, cheats, save,
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
const MINUTES: u16 = 24 * 60;

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

/// A fish rising to the surface or sinking to the bed to sleep moves
/// straight up or down, but sleeps level, not nose-up or nose-down.
#[test]
fn a_swimmer_that_rises_or_sinks_to_rest_sleeps_level() {
    let mut w = world(13);
    let betta = add(&mut w, "betta", 60, 50);
    let goldfish = add(&mut w, "common_goldfish", 150, 15);
    let tetra = add(&mut w, "neon_tetra", 100, 30);
    w.set_time_of_day(MIDNIGHT);
    let start: Vec<_> = [betta, goldfish]
        .iter()
        .map(|&id| w.creature(id).unwrap().centre().1)
        .collect();
    let mut steepest = 0;
    for _ in 0..90 * TICKS_PER_SECOND {
        w.tick();
        for id in [betta, goldfish, tetra] {
            steepest = steepest.max(w.creature(id).unwrap().pitch().abs());
        }
    }
    for (id, from) in [betta, goldfish].into_iter().zip(start) {
        let fish = w.creature(id).unwrap();
        assert!(fish.asleep());
        assert!((fish.centre().1 - from).abs() > 10.0, "it swam to its spot");
        assert_eq!(fish.pitch(), 0, "it sleeps level");
    }
    assert_eq!(w.creature(tetra).unwrap().pitch(), 0);
    assert!(steepest > 0, "it did pitch on the way");
    // And it stays level through the night.
    run(&mut w, 60);
    assert!(
        [betta, goldfish, tetra]
            .iter()
            .all(|&id| w.creature(id).unwrap().pitch() == 0)
    );
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
    // The woken betta patrols its territory and may chase the tetra up
    // again (a fright restarts its time awake), so allow a few spells.
    let settled = (0..3 * u32::from(WOKEN_TICKS)).any(|_| {
        w.tick();
        asleep(&w, tetra)
    });
    assert!(settled, "then settles again");
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

#[test]
fn a_hunting_predator_wakes_sleeping_prey_and_a_sleeping_one_hunts_nothing() {
    // Outside easy mode, so the betta hunts. It is a surface fish that
    // rests at the top, so its shrimp prey sleeps on the bed below.
    let mut w = world(31);
    w.set_easy_mode(false);
    let shrimp = add(&mut w, "cherry_shrimp", 100, 68);
    let betta = add(&mut w, "betta", 100, 10);
    w.set_creature_energy(betta, 1_000);
    w.set_time_of_day(MIDNIGHT);
    run(&mut w, 10);
    assert!(asleep(&w, betta) && asleep(&w, shrimp));
    assert!(
        w.creature(betta).unwrap().quarry().is_none(),
        "asleep, it hunts nothing"
    );
    // Wake the betta right over the shrimp: the shrimp takes fright and wakes.
    let (x, y) = w.creature(shrimp).unwrap().cell();
    assert!(w.move_creature(betta, x, y - 8));
    w.release_creature(betta);
    assert!(w.hold_creature(betta), "the net wakes the betta");
    w.release_creature(betta);
    let mut woke = false;
    for _ in 0..TICKS_PER_SECOND {
        w.tick();
        let Some(s) = w.creature(shrimp) else {
            woke = true; // eaten: it was awake enough to be hunted
            break;
        };
        if !s.asleep() {
            woke = true;
            break;
        }
    }
    assert!(woke, "a predator overhead wakes the shrimp");
}

/// Whether any creature of `ids` is asleep at `minute`, under `w`'s schedule.
fn count_asleep(w: &mut World, ids: &[CreatureId], minute: u16) -> usize {
    w.set_time_of_day(minute);
    w.tick();
    ids.iter().filter(|&&id| asleep(w, id)).count()
}

fn sleepers(seed: u64) -> (World, Vec<CreatureId>) {
    let mut w = world(seed);
    let ids = (0..8)
        .map(|i| add(&mut w, "neon_tetra", 20 + 20 * i, 30))
        .collect();
    (w, ids)
}

#[test]
fn the_fixed_schedule_is_the_default() {
    let (w, _) = community(1);
    assert_eq!(w.daylight_schedule(), DaylightSchedule::FIXED);
}

#[test]
fn sleep_follows_a_winter_schedule() {
    // Late sunrise (08:00) and early sunset (16:00): rest from 16:30 to
    // 07:30 plus each creature's stagger.
    let (mut w, ids) = sleepers(5);
    w.set_daylight_schedule(DaylightSchedule::from_sun_times(8 * 60, 16 * 60));
    assert_eq!(w.daylight_schedule().bedtime, 16 * 60 + 30);
    let all = ids.len();
    // Early evening is still awake; just after the latest bedtime all are
    // asleep (the fixed schedule would have them up until 21:00).
    assert_eq!(count_asleep(&mut w, &ids, 16 * 60), 0);
    assert_eq!(
        count_asleep(&mut w, &ids, 16 * 60 + 30 + STAGGER_MINUTES),
        all
    );
    assert_eq!(count_asleep(&mut w, &ids, 20 * 60), all);
    // Still asleep at 06:00 (fixed wake time) and 07:29, awake once past
    // the latest wake time.
    assert_eq!(count_asleep(&mut w, &ids, 6 * 60 + STAGGER_MINUTES), all);
    assert_eq!(count_asleep(&mut w, &ids, 7 * 60 + 29), all);
    assert_eq!(count_asleep(&mut w, &ids, 7 * 60 + 30 + STAGGER_MINUTES), 0);
    assert_eq!(count_asleep(&mut w, &ids, NOON), 0);
}

#[test]
fn sleep_follows_a_schedule_that_wraps_midnight() {
    // Rest from 01:00 to 09:00, never crossing midnight: a night-shift sun.
    let (mut w, ids) = sleepers(5);
    let mut s = DaylightSchedule::from_sun_times(9 * 60 + 30, 0);
    s.bedtime = 60;
    s.wake_time = 9 * 60 - 30;
    w.set_daylight_schedule(s);
    let all = ids.len();
    assert_eq!(count_asleep(&mut w, &ids, 0), 0);
    assert_eq!(count_asleep(&mut w, &ids, 60 + STAGGER_MINUTES), all);
    assert_eq!(count_asleep(&mut w, &ids, 8 * 60), all);
    assert_eq!(count_asleep(&mut w, &ids, 9 * 60 + STAGGER_MINUTES), 0);
    assert_eq!(count_asleep(&mut w, &ids, 22 * 60), 0);
    // And a bedtime before midnight, wake after: 23:00 to 05:00.
    s.bedtime = 23 * 60;
    s.wake_time = 5 * 60 - STAGGER_MINUTES;
    w.set_daylight_schedule(s);
    assert_eq!(count_asleep(&mut w, &ids, 23 * 60 + STAGGER_MINUTES), all);
    assert_eq!(count_asleep(&mut w, &ids, 2 * 60), all);
    assert_eq!(count_asleep(&mut w, &ids, 5 * 60), 0);
    assert_eq!(count_asleep(&mut w, &ids, 12 * 60), 0);
}

#[test]
fn a_short_night_never_means_rest_all_day() {
    // Sunrise 00:10 and sunset 23:50: the rest window has no length, so
    // staggers give at most an hour or so of rest around midnight.
    let (mut w, ids) = sleepers(5);
    w.set_daylight_schedule(DaylightSchedule::from_sun_times(10, 23 * 60 + 50));
    for m in (2 * 60..22 * 60).step_by(30) {
        assert_eq!(count_asleep(&mut w, &ids, m), 0, "{m}");
    }
}

#[test]
fn polar_day_keeps_everyone_up_and_polar_night_keeps_the_default_rest() {
    // Tromsø.
    let (mut w, ids) = sleepers(9);
    let june = DaylightSchedule::solar_schedule(172, 69.65, 18.96, 120);
    assert_eq!(june.polar, PolarDaylight::PolarDay);
    w.set_daylight_schedule(june);
    for m in (0..MINUTES).step_by(60) {
        assert_eq!(count_asleep(&mut w, &ids, m), 0, "midnight sun at {m}");
    }

    let december = DaylightSchedule::solar_schedule(355, 69.65, 18.96, 60);
    assert_eq!(december.polar, PolarDaylight::PolarNight);
    w.set_daylight_schedule(december);
    let all = ids.len();
    assert_eq!(count_asleep(&mut w, &ids, LATE), all);
    assert_eq!(count_asleep(&mut w, &ids, MIDNIGHT), all);
    assert_eq!(count_asleep(&mut w, &ids, MORNING), 0);
    // Noon is dark, but they are up all the same.
    assert_eq!(count_asleep(&mut w, &ids, NOON), 0);
}
