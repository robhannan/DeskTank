//! SPEC T9 part 2 acceptance tests (§2.4 *T9 part 2*): livebearers and egg
//! layers breed when mature, well fed, rested and paired, slower in a
//! crowded tank and never past the population cap; clutches hatch into fry
//! or rot at the cap; colour morphs breed true in same-morph lines and
//! revert towards wildtype in mixed ones over generations (the seeded
//! multi-generation test); and predators eat fry and shrimp they touch,
//! but not larger creatures, and not in easy mode.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use desktank_sim::{
    Brush, CreatureId, LIFE_DAY_TICKS, LifeStage, MaterialId, MaterialTable, Rect, SETTLE_TICKS,
    SpeciesId, TICKS_PER_SECOND, Traits, World, WorldConfig,
};

fn world(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: 128,
            height: 96,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let sand = id(&w, "sand");
    w.fill_rect(rect(0, 88, 128, 96), sand, Brush::Replace)
        .unwrap();
    w
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn morph(w: &World, s: SpeciesId, name: &str) -> u8 {
    w.species().get(s).unwrap().morph(name).unwrap()
}

/// An adult of sex `female` with two `alleles` (`None`: the palette's).
fn adult(female: bool, alleles: Option<[u8; 2]>) -> Traits {
    Traits {
        female: Some(female),
        alleles,
        ..Traits::default()
    }
}

/// Ticks until `id` carries young, at most `limit`.
fn ticks_to_carry(w: &mut World, id: CreatureId, limit: u32) -> Option<u32> {
    (1..=limit).find(|_| {
        w.tick();
        w.creature(id).is_some_and(|c| c.carrying() > 0)
    })
}

/// A male and a female of `name`, settled in.
fn pair(w: &mut World, name: &str) -> (CreatureId, CreatureId) {
    let s = species(w, name);
    let (y, x) = (if name == "corydoras" { 84 } else { 50 }, 60);
    let male = w
        .add_creature_with_traits(s, x, y, adult(false, None))
        .unwrap();
    let female = w
        .add_creature_with_traits(s, x + 12, y, adult(true, None))
        .unwrap();
    w.skip_breeding_time(SETTLE_TICKS);
    (male, female)
}

#[test]
fn the_palette_adds_pairs_and_new_arrivals_settle_first() {
    let mut w = world(1);
    let s = species(&w, "guppy");
    let ids: Vec<_> = (0..4)
        .map(|i| w.add_creature(s, 20 + 20 * i, 40).unwrap())
        .collect();
    let sexes: Vec<_> = ids
        .iter()
        .map(|&i| w.creature(i).unwrap().female())
        .collect();
    assert_eq!(sexes, [false, true, false, true]);
    for &i in &ids {
        let c = w.creature(i).unwrap();
        assert_eq!(c.rest(), SETTLE_TICKS);
        let wild = w.species().get(s).unwrap().palette_morph;
        assert_eq!((c.alleles(), w.creature_morph(c)), ([wild; 2], wild));
    }
    // An unknown morph is refused.
    assert!(
        w.add_creature_with_traits(s, 30, 30, adult(true, Some([0, 9])))
            .is_err()
    );
    // A day later none has bred yet, as they settled in.
    for _ in 0..SETTLE_TICKS - 1 {
        w.tick();
    }
    assert!(w.creatures().iter().all(|c| c.carrying() == 0));
}

#[test]
fn a_livebearer_carries_and_gives_birth_to_fry() {
    let mut w = world(2);
    let (male, female) = pair(&mut w, "guppy");
    let s = species(&w, "guppy");
    let red = morph(&w, s, "red");
    // A red father: the young are red/wild carriers, shown as wild.
    w.remove_creature(male);
    let father = w
        .add_creature_with_traits(s, 50, 40, adult(false, Some([red, red])))
        .unwrap();
    let t = ticks_to_carry(&mut w, female, 30_000).expect("she conceives");
    let gestation = 28 * LIFE_DAY_TICKS;
    let mum = w.creature(female).unwrap();
    assert!(mum.carrying() > gestation - 100, "{t}");
    assert_eq!(w.creature(father).unwrap().carrying(), 0);
    w.skip_breeding_time(gestation);
    let before = w.creatures().len();
    w.tick();
    let fry: Vec<_> = w
        .creatures()
        .iter()
        .filter(|c| c.age() <= 1)
        .cloned()
        .collect();
    assert_eq!(w.creatures().len(), before + fry.len());
    assert!((3..=8).contains(&fry.len()), "{}", fry.len());
    let rest = 7 * LIFE_DAY_TICKS;
    let mum = w.creature(female).unwrap();
    assert_eq!((mum.carrying(), mum.rest()), (0, rest));
    let wild = w.species().get(s).unwrap().wildtype;
    let (mut hets, mut females) = (0, 0);
    for c in &fry {
        assert_eq!(w.life_stage(c), LifeStage::Fry);
        assert_eq!(c.species(), s);
        // Near their mother.
        let (mx, my) = mum.centre();
        let (x, y) = c.centre();
        assert!((x - mx).abs() < 12.0 && (y - my).abs() < 12.0);
        hets += usize::from(c.alleles() == [wild, red]);
        females += usize::from(c.female());
    }
    assert!(hets + 1 >= fry.len(), "allow a rare mutation");
    assert!(females > 0 && females < fry.len() || fry.len() < 4);
    // She rests before the next brood, then breeds again.
    assert_eq!(ticks_to_carry(&mut w, female, 2 * LIFE_DAY_TICKS), None);
}

#[test]
fn breeding_needs_a_mature_fed_ready_pair() {
    let limit = 12 * 60 * TICKS_PER_SECOND;
    // Two females, no male.
    let mut w = world(3);
    let s = species(&w, "guppy");
    let a = w
        .add_creature_with_traits(s, 40, 40, adult(true, None))
        .unwrap();
    w.add_creature_with_traits(s, 80, 40, adult(true, None))
        .unwrap();
    w.skip_breeding_time(SETTLE_TICKS);
    assert_eq!(ticks_to_carry(&mut w, a, limit), None);
    // A juvenile female.
    let mut w = world(3);
    w.add_creature_with_traits(s, 40, 40, adult(false, None))
        .unwrap();
    let young = w
        .add_creature_with_traits(
            s,
            80,
            40,
            Traits {
                age: Some(40 * LIFE_DAY_TICKS),
                ..adult(true, None)
            },
        )
        .unwrap();
    w.skip_breeding_time(SETTLE_TICKS);
    assert_eq!(ticks_to_carry(&mut w, young, limit), None);
    // A hungry female (under half her reserve), until she is fed.
    let mut w = world(3);
    let (_, female) = pair(&mut w, "guppy");
    let reserve = desktank_sim::ENERGY_RESERVE;
    w.set_creature_energy(female, reserve / 2 - 1);
    assert_eq!(ticks_to_carry(&mut w, female, limit), None);
    w.set_creature_energy(female, reserve);
    assert!(ticks_to_carry(&mut w, female, 3 * limit).is_some());
    // Species that don't breed in fresh water never do.
    for name in ["nerite", "amano_shrimp", "thai_micro_crab"] {
        let mut w = world(4);
        let s = species(&w, name);
        assert!(w.species().get(s).unwrap().breeding.is_none());
        let _ = pair(&mut w, name);
        for _ in 0..limit {
            w.tick();
        }
        assert_eq!(w.creatures().len(), 2, "{name}");
        assert_eq!(w.clutches().len(), 0);
    }
}

/// Mean ticks until a ready guppy pair conceives, over a few seeds, in a
/// tank holding `others` more creatures under a cap of `cap`.
fn mean_ticks_to_conceive(others: usize, cap: usize) -> f64 {
    let seeds = 8;
    let mut total = 0;
    for seed in 0..seeds {
        let mut w = world(100 + seed);
        w.set_creature_cap(cap);
        let (_, female) = pair(&mut w, "guppy");
        // Males of a species that doesn't breed, so only the pair counts.
        let nerite = species(&w, "nerite");
        for i in 0..others {
            let x = 4 + (i % 30) as i64 * 4;
            let y = 88 - (i / 30) as i64 * 6;
            w.add_creature(nerite, x, y).unwrap();
        }
        total += ticks_to_carry(&mut w, female, 400_000).unwrap_or(400_000);
    }
    f64::from(total) / seeds as f64
}

#[test]
fn a_crowded_tank_breeds_slower_and_a_full_one_not_at_all() {
    let roomy = mean_ticks_to_conceive(0, 60);
    let crowded = mean_ticks_to_conceive(48, 60);
    // Five times less room: about five times slower.
    assert!(crowded > 2.5 * roomy, "{roomy} {crowded}");
    // At the cap she never conceives.
    let mut w = world(5);
    let (_, female) = pair(&mut w, "guppy");
    w.set_creature_cap(2);
    assert_eq!(ticks_to_carry(&mut w, female, 30_000), None);
    // A brood born into a nearly full tank is cut to the room left.
    w.set_creature_cap(60);
    ticks_to_carry(&mut w, female, 60_000).expect("she conceives");
    w.set_creature_cap(4);
    w.skip_breeding_time(28 * LIFE_DAY_TICKS);
    w.tick();
    assert_eq!(w.creatures().len(), 4);
}

#[test]
fn an_egg_layer_lays_a_clutch_that_hatches_into_fry() {
    let mut w = world(6);
    let (_, female) = pair(&mut w, "corydoras");
    let s = species(&w, "corydoras");
    let egg = id(&w, "egg");
    let water = id(&w, "water");
    let mut laid = false;
    for _ in 0..40_000 {
        w.tick();
        if !w.clutches().is_empty() {
            laid = true;
            break;
        }
    }
    assert!(laid, "she lays");
    // She doesn't carry them: she rests at once.
    let mum = w.creature(female).unwrap();
    assert_eq!(mum.carrying(), 0);
    assert_eq!(mum.rest(), 14 * LIFE_DAY_TICKS);
    let clutch = w.clutches()[0].clone();
    assert_eq!(clutch.species(), s);
    // Up to her brood of 4–10, as many as there are spots for.
    assert!((1..=10).contains(&clutch.eggs().len()));
    let width = w.width();
    for &e in clutch.eggs() {
        let (x, y) = (e as usize % width, e as usize / width);
        let cell = w.cell(x, y).unwrap();
        assert_eq!((cell.material, cell.state), (egg, clutch.id()));
        // Near her (on the bed below her at most) and on no creature.
        let b = w.creature_body(mum);
        assert!(
            x + 7 > b.x0 && x < b.x1 + 7 && y + 7 > b.y0,
            "{x} {y} {b:?}"
        );
        assert!(w.creature_at(x as i64, y as i64).is_none());
    }
    // Most stick to something: here the sand bed below them.
    let on_bed = clutch
        .eggs()
        .iter()
        .filter(|&&e| {
            let (x, y) = (e as usize % width, e as usize / width);
            w.cell(x, y + 1).unwrap().material != water
        })
        .count();
    assert!(on_bed * 2 >= clutch.eggs().len(), "{on_bed}");
    // Nothing happens until it is due; then each egg hatches into a fry.
    for _ in 0..100 {
        w.tick();
    }
    assert_eq!(w.clutches().len(), 1);
    let before = w.creatures().len();
    w.skip_breeding_time(4 * LIFE_DAY_TICKS);
    for _ in 0..TICKS_PER_SECOND {
        w.tick();
    }
    assert_eq!(w.clutches().len(), 0);
    let born = w.creatures().len() - before;
    assert_eq!(born, clutch.eggs().len());
    for &e in clutch.eggs() {
        let (x, y) = (e as usize % width, e as usize / width);
        assert_ne!(w.cell(x, y).unwrap().material, egg);
    }
    let fry = w.creatures().iter().filter(|c| c.age() < 30).count();
    assert_eq!(fry, born);
}

#[test]
fn eggs_rot_when_the_tank_is_full_and_lost_eggs_dont_hatch() {
    let mut w = world(7);
    let _ = pair(&mut w, "corydoras");
    for _ in 0..40_000 {
        w.tick();
        if !w.clutches().is_empty() {
            break;
        }
    }
    let clutch = w.clutches()[0].clone();
    let width = w.width();
    // One egg is wiped away (the erase tool, a water change).
    let lost = clutch.eggs()[0] as usize;
    let water = id(&w, "water");
    w.set_cell(lost % width, lost / width, water).unwrap();
    // The tank fills to its cap: one fry fits, the rest rot.
    let cap = w.creatures().len() + 1;
    w.set_creature_cap(cap);
    let detritus = id(&w, "detritus");
    let before = w.material_counts()[usize::from(detritus.0)];
    w.skip_breeding_time(4 * LIFE_DAY_TICKS);
    for _ in 0..TICKS_PER_SECOND {
        w.tick();
    }
    assert_eq!(w.creatures().len(), cap);
    let rotted = w.material_counts()[usize::from(detritus.0)] - before;
    assert_eq!(rotted, clutch.eggs().len() - 2);
}

/// SPEC T9 acceptance, "the population never exceeds the cap", over a run
/// that breeds: four pairs (two egg layers, a livebearer and a shrimp)
/// under a cap of 10, fed and with their breeding clocks hurried along,
/// for five life days. Every tick the tank holds at most 10 creatures and
/// at most `MAX_CLUTCHES` clutches, and it does fill up.
#[test]
fn breeding_pairs_never_take_the_tank_past_its_cap() {
    let mut w = world(4);
    w.set_creature_cap(10);
    for (i, name) in ["neon_tetra", "harlequin_rasbora", "guppy", "cherry_shrimp"]
        .into_iter()
        .enumerate()
    {
        let s = species(&w, name);
        let (x, y) = (
            10 + 28 * i as i64,
            if name == "cherry_shrimp" { 84 } else { 40 },
        );
        for (dx, female) in [(0, false), (8, true)] {
            w.add_creature_with_traits(s, x + dx, y, adult(female, None))
                .unwrap();
        }
    }
    w.skip_breeding_time(SETTLE_TICKS);
    let (mut most, mut clutches) = (0, 0);
    for t in 0..5 * LIFE_DAY_TICKS {
        if t % (LIFE_DAY_TICKS / 8) == 0 {
            let ids: Vec<_> = w
                .creatures()
                .iter()
                .map(desktank_sim::Creature::id)
                .collect();
            for id in ids {
                w.set_creature_energy(id, desktank_sim::ENERGY_RESERVE);
            }
            w.skip_breeding_time(7 * LIFE_DAY_TICKS);
        }
        w.tick();
        let n = w.creatures().len();
        assert!(n <= 10, "{n} creatures on tick {t}");
        assert!(w.clutches().len() <= desktank_sim::MAX_CLUTCHES);
        most = most.max(n);
        clutches = clutches.max(w.clutches().len());
    }
    assert_eq!(most, 10, "the tank never filled");
    assert!(clutches > 0, "no clutch was laid");
}

/// One generation of cherry shrimp bred in `w`, which carries on from the
/// last: the tank is emptied of creatures (last generation's parents and
/// any young not kept), `parents` (sex, alleles) are put back as settled
/// adults, and the tank runs until they have at least `want` young, whose
/// alleles and sexes are returned in birth order. The young are put back
/// grown rather than raised for the 75 life days a cherry shrimp takes to
/// mature (900,000 ticks a generation), which the test can't afford.
fn breed_generation(
    w: &mut World,
    parents: &[(bool, [u8; 2])],
    want: usize,
) -> Vec<(bool, [u8; 2])> {
    let s = species(w, "cherry_shrimp");
    let old: Vec<_> = w
        .creatures()
        .iter()
        .map(desktank_sim::Creature::id)
        .collect();
    for id in old {
        w.remove_creature(id);
    }
    for (i, &(female, alleles)) in parents.iter().enumerate() {
        let x = 6 + (i as i64 % 12) * 10;
        w.add_creature_with_traits(s, x, 84, adult(female, Some(alleles)))
            .unwrap();
    }
    let parents_n = parents.len();
    w.skip_breeding_time(SETTLE_TICKS);
    for _ in 0..200_000 {
        w.tick();
        if w.creatures().iter().any(|c| c.carrying() > 0) {
            w.skip_breeding_time(30 * LIFE_DAY_TICKS);
        }
        if w.creatures().len() >= parents_n + want {
            break;
        }
    }
    let young: Vec<_> = w.creatures()[parents_n..]
        .iter()
        .map(|c| (c.female(), c.alleles()))
        .collect();
    assert!(young.len() >= want, "{} young", young.len());
    young
}

/// The next parents: eight females and four males from the young, spread
/// evenly through the birth order so that they come from many broods (no
/// selection by colour).
fn next_parents(young: &[(bool, [u8; 2])]) -> Vec<(bool, [u8; 2])> {
    let spread = |female: bool, n: usize| {
        let of: Vec<_> = young.iter().filter(|y| y.0 == female).copied().collect();
        assert!(of.len() >= n, "{} young of a sex", of.len());
        (0..n)
            .map(move |i| of[i * of.len() / n])
            .collect::<Vec<_>>()
    };
    let mut out = spread(true, 8);
    out.extend(spread(false, 4));
    out
}

/// The seeded multi-generation test (SPEC T9 acceptance): cherry shrimp
/// lines bred on in one tank for five generations each, with no selection
/// by colour (see `breed_generation` for what carries over). Same-morph
/// lines (red, blue) breed true; a red × blue cross gives wildtype (brown)
/// young, and the mixed line keeps showing wildtype in a large share.
#[test]
fn same_morph_lines_breed_true_and_mixed_lines_revert_to_wildtype() {
    let w = world(0);
    let s = species(&w, "cherry_shrimp");
    let sp = w.species().get(s).unwrap();
    let (red, blue, wild) = (
        sp.morph("red").unwrap(),
        sp.morph("blue").unwrap(),
        sp.wildtype,
    );
    assert_eq!(sp.morphs[usize::from(wild)], "wild");
    let shows = |a: [u8; 2]| if a[0] == a[1] { a[0] } else { wild };
    let share = |young: &[(bool, [u8; 2])], m: u8| {
        young.iter().filter(|y| shows(y.1) == m).count() as f64 / young.len() as f64
    };
    let line = |start: Vec<(bool, [u8; 2])>, seed: u64| {
        let mut w = world(seed);
        let mut parents = start;
        let mut shares = Vec::new();
        for _ in 0..5 {
            let young = breed_generation(&mut w, &parents, 36);
            shares.push(young.clone());
            parents = next_parents(&young);
        }
        shares
    };
    let pure = |m: u8| {
        let mut p = vec![(true, [m, m]); 8];
        p.extend([(false, [m, m]); 4]);
        p
    };
    // A mutant among the twelve parents can leave a quarter of the next
    // generation showing wildtype, so the floor per generation is lower
    // than the line's mean.
    for (m, seed) in [(red, 10), (blue, 20)] {
        let shares: Vec<_> = line(pure(m), seed).iter().map(|y| share(y, m)).collect();
        let mean = shares.iter().sum::<f64>() / 5.0;
        assert!(
            shares.iter().all(|&s| s >= 0.7) && mean >= 0.85,
            "morph {m}: {shares:?}"
        );
    }
    let mut mixed = vec![(true, [red, red]); 8];
    mixed.extend([(false, [blue, blue]); 4]);
    let gens = line(mixed, 30);
    let wild_share: Vec<_> = gens.iter().map(|y| share(y, wild)).collect();
    assert!(wild_share[0] >= 0.9, "F1 {wild_share:?}");
    // Random mating keeps about half the line mixed, so wildtype (as the
    // pure lines almost never show it); drift in a 12-parent tank wanders
    // around that.
    let later = wild_share[1..].iter().sum::<f64>() / 4.0;
    assert!(
        wild_share[1..].iter().all(|&w| w >= 0.25) && later >= 0.35,
        "{wild_share:?}"
    );
}

#[test]
fn a_hungry_predator_eats_small_prey_it_touches() {
    // A shallow tank, about 14 rows of water: room for the betta's 12-cell
    // body (though below its `min_depth`), so the betta (surface) and
    // shrimp (bottom) meet.
    let mut w = World::new(
        WorldConfig {
            width: 40,
            height: 16,
            seed: 8,
            water_level: 0.9,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let betta = species(&w, "betta");
    let shrimp = species(&w, "cherry_shrimp");
    let hunter = w.add_creature(betta, 20, 7).unwrap();
    let prey = w.add_creature(shrimp, 20, 14).unwrap();
    let energy = desktank_sim::ENERGY_RESERVE / 2;
    w.set_creature_energy(hunter, energy);
    let eaten = (1..=120 * TICKS_PER_SECOND).find(|_| {
        w.tick();
        w.creature(prey).is_none()
    });
    assert!(eaten.is_some(), "the betta eats the shrimp");
    assert!(w.corpses().is_empty(), "eaten whole");
    let b = w.creature(hunter).unwrap();
    assert_eq!(b.eaten(), 1);
    assert!(b.energy() > energy);
    assert!(!b.hungry());
}

#[test]
fn predators_leave_larger_creatures_alone_and_easy_mode_stops_them() {
    let shallow = |seed| {
        World::new(
            WorldConfig {
                width: 40,
                height: 16,
                seed,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap()
    };
    let minutes = 3 * 60 * TICKS_PER_SECOND;
    // An amano (4 cm) is too big for the betta (2.5 cm prey).
    let mut w = shallow(9);
    w.add_creature(species(&w, "betta"), 20, 7).unwrap();
    w.add_creature(species(&w, "amano_shrimp"), 20, 14).unwrap();
    for _ in 0..minutes {
        w.tick();
    }
    assert_eq!(w.creatures().len(), 2);
    // A cherry it would eat survives in easy mode.
    let mut w = shallow(8);
    w.set_easy_mode(true);
    w.add_creature(species(&w, "betta"), 20, 7).unwrap();
    w.add_creature(species(&w, "cherry_shrimp"), 20, 14)
        .unwrap();
    for _ in 0..minutes {
        w.tick();
    }
    assert_eq!(w.creatures().len(), 2);
}

#[test]
fn guppies_eat_their_newborn_fry_but_not_grown_ones() {
    let mut w = world(12);
    let s = species(&w, "guppy");
    // A walled-off corner keeps them together.
    let rock = id(&w, "rock");
    w.fill_rect(rect(40, 0, 42, 88), rock, Brush::Replace)
        .unwrap();
    w.add_creature(s, 20, 40).unwrap();
    let newborn: Vec<_> = (0..4)
        .map(|i| w.add_creature_aged(s, 8 + i * 8, 30, 0).unwrap())
        .collect();
    let grown = w.add_creature_aged(s, 30, 50, 30 * LIFE_DAY_TICKS).unwrap();
    for _ in 0..10 * 60 * TICKS_PER_SECOND {
        w.tick();
    }
    let left = newborn.iter().filter(|&&f| w.creature(f).is_some()).count();
    assert!(left < newborn.len(), "some fry were eaten");
    assert!(w.creature(grown).is_some(), "a juvenile is too big");
}

#[test]
fn breeding_is_deterministic_and_survives_a_save() {
    let run = |seed| {
        let mut w = world(seed);
        let _ = pair(&mut w, "guppy");
        let _ = pair(&mut w, "corydoras");
        for t in 0..30_000 {
            w.tick();
            if t % 5_000 == 0 {
                w.skip_breeding_time(10 * LIFE_DAY_TICKS);
            }
        }
        w
    };
    let a = run(13);
    let b = run(13);
    assert_eq!(a.state_hash(), b.state_hash());
    let mut loaded = desktank_sim::save::decode(
        &desktank_sim::save::encode(&a),
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    assert_eq!(loaded.state_hash(), a.state_hash());
    let mut a = a;
    for _ in 0..2_000 {
        a.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), a.state_hash());
}
