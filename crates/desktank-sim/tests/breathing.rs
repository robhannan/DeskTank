//! Air-breathers under a covered surface (SPEC §2.4): a breather that can't
//! reach air builds up stress and, a frog quickly, suffocates; one finds
//! a gap in the mat and breathes there; none freezes in place under it;
//! and a covered surface is flagged as an unmet need.
#![allow(clippy::cast_possible_wrap)]

use desktank_sim::{
    Brush, CreatureId, DeathCause, MaterialTable, Rect, SpeciesId, TICKS_PER_SECOND, UnmetNeed,
    World, WorldConfig,
};

const WIDTH: usize = 200;
const HEIGHT: usize = 90;
const BED: usize = 6;

fn seconds(s: u32) -> u32 {
    s * TICKS_PER_SECOND
}

/// A sand-bedded tank whose top water row is covered, except for `gap`
/// (columns), with roots hanging six rows below every other column. The
/// cover uses full-depth solid device cells, as dense as a thick floating mat: painted floating
/// leaves are a plant per stroke in one depth band each, which a breather
/// in another band slips past (as it should).
fn matted(seed: u64, gap: Option<(usize, usize)>) -> World {
    let mut w = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level: 0.9,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let id = |w: &World, name| w.materials().id(name).unwrap();
    let (sand, cover, root) = (id(&w, "sand"), id(&w, "device"), id(&w, "root"));
    let rect = |x0, y0, x1, y1| Rect { x0, y0, x1, y1 };
    w.fill_rect(rect(0, HEIGHT - BED, WIDTH, HEIGHT), sand, Brush::Replace)
        .unwrap();
    let s = w.surface_row();
    let (a, b) = gap.unwrap_or((WIDTH, WIDTH));
    for (x0, x1) in [(0, a), (b, WIDTH)].into_iter().filter(|(x0, x1)| x0 < x1) {
        w.fill_rect(rect(x0, s, x1, s + 1), cover, Brush::Replace)
            .unwrap();
    }
    for x in (0..WIDTH).filter(|&x| x % 2 == 0 && !(a..b).contains(&x)) {
        w.fill_rect(rect(x, s + 1, x + 1, s + 7), root, Brush::Replace)
            .unwrap();
    }
    w
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

/// Adds a `name` under the mat at column `x`.
fn add(w: &mut World, name: &str, x: i64) -> CreatureId {
    let y = w.surface_row() as i64 + 20;
    w.add_creature(species(w, name), x, y).unwrap()
}

/// Ticks until `id` dies, up to `limit`.
fn ticks_to_death(w: &mut World, id: CreatureId, limit: u32) -> Option<u32> {
    (1..=limit).find(|_| {
        w.tick();
        w.creature(id).is_none()
    })
}

#[test]
fn the_mat_stays_put() {
    let mut w = matted(1, None);
    let leaf = w.materials().id("device").unwrap();
    for _ in 0..seconds(60) {
        w.tick();
    }
    let s = w.surface_row();
    let covered = (0..WIDTH)
        .filter(|&x| w.cell(x, s).unwrap().material == leaf)
        .count();
    assert_eq!(covered, WIDTH);
}

#[test]
fn a_frog_under_a_full_mat_suffocates_within_minutes() {
    let mut w = matted(2, None);
    let frog = add(&mut w, "african_dwarf_frog", 100);
    // A minute in: overdue, building stress, but alive.
    assert_eq!(ticks_to_death(&mut w, frog, seconds(80)), None);
    let stress = w.creature(frog).unwrap().stress();
    assert!(stress > 0, "{stress}");
    // It goes within its 45 s grace and two-minute tolerance (and the
    // first 20 s before a breath is due).
    ticks_to_death(&mut w, frog, seconds(150)).expect("suffocates");
    let corpse = w.corpses().last().unwrap();
    assert_eq!(corpse.cause(), DeathCause::Suffocation);
}

#[test]
fn a_gourami_under_a_full_mat_wanders_and_lasts_longer() {
    let mut w = matted(3, None);
    let fish = add(&mut w, "honey_gourami", 100);
    let mut cells = std::collections::BTreeSet::new();
    for _ in 0..300 {
        for _ in 0..TICKS_PER_SECOND {
            w.tick();
        }
        cells.insert(w.creature(fish).expect("alive after five minutes").cell());
    }
    // It doesn't hold still pushing at the mat: it wanders.
    assert!(cells.len() > 100, "{} spots", cells.len());
    let stress = w.creature(fish).unwrap().stress();
    assert!(
        stress > 0 && stress < desktank_sim::STRESS_LIMIT / 2,
        "{stress}"
    );
}

#[test]
fn a_gourami_finds_the_one_gap_and_breathes() {
    let gap = (170, 190);
    let mut w = matted(4, Some(gap));
    let fish = add(&mut w, "honey_gourami", 20);
    let s = w.surface_row();
    let breathed = (0..seconds(120)).any(|_| {
        w.tick();
        let b = w.creature_body(w.creature(fish).unwrap());
        b.y0 == s && b.x0 >= gap.0 && b.x1 <= gap.1
    });
    assert!(breathed, "never surfaced in the gap");
    // Breathing there now and then, it stays unstressed.
    for _ in 0..seconds(300) {
        w.tick();
    }
    let c = w.creature(fish).expect("alive");
    assert_eq!(c.stress(), 0);
}

#[test]
fn a_covered_surface_is_an_unmet_need() {
    let covered = matted(5, None);
    let betta = species(&covered, "betta");
    assert_eq!(covered.unmet_need(betta), Some(UnmetNeed::Air));
    assert_eq!(covered.unmet_need(species(&covered, "corydoras")), None);
    // A gap narrower than a betta is no help; a wide one is.
    let wide = covered.species().get(betta).unwrap().size().0;
    let narrow = matted(5, Some((50, 50 + wide - 1)));
    assert_eq!(narrow.unmet_need(betta), Some(UnmetNeed::Air));
    let open = matted(5, Some((50, 50 + wide)));
    assert_eq!(open.unmet_need(betta), None);
}
