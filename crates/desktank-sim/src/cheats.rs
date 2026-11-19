//! Cheats (SPEC §2.5): the Konami code (↑ ↑ ↓ ↓ ← → ← → B A) opens a
//! text field, and the line typed there runs one command from a small
//! registry.
//!
//! This is the platform-free half, shared by every host: a host maps its
//! key presses to [`CheatKey`] and feeds them to a [`Konami`] detector,
//! shows its own text field when the detector fires, and passes the
//! submitted line to [`run`]. Hosts that see keys only while the interact
//! chord is held (the overlay, SPEC §3.3) build the detector with
//! [`Konami::with_chord`] and say whether the chord is held with each key;
//! hosts with keyboard focus (the window, the web) use [`Konami::new`].
//!
//! Cheats are harmless shortcuts to things the tools can already do (feed,
//! stock, change the water), plus two easter eggs: the phrases `why not?`
//! and `home owner!` each add one of the cheat-only crabs
//! ([`MEDICAL_CRAB`], [`HOME_OWNING_CRAB`]), which the palette and `spawn`
//! don't offer. An unknown or malformed line returns a [`CheatError`] for
//! the host to show, and never panics.

use crate::{Brush, MaterialId, World, WorldError};

/// A key the Konami detector cares about; hosts map every other key to
/// [`CheatKey::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheatKey {
    /// The up arrow.
    Up,
    /// The down arrow.
    Down,
    /// The left arrow.
    Left,
    /// The right arrow.
    Right,
    /// The B key (either case).
    B,
    /// The A key (either case).
    A,
    /// Any other key.
    Other,
}

/// The Konami code.
pub const KONAMI: [CheatKey; 10] = [
    CheatKey::Up,
    CheatKey::Up,
    CheatKey::Down,
    CheatKey::Down,
    CheatKey::Left,
    CheatKey::Right,
    CheatKey::Left,
    CheatKey::Right,
    CheatKey::B,
    CheatKey::A,
];

/// Watches key presses for the Konami code.
///
/// Feed it key presses only, without auto-repeats. A wrong key doesn't
/// always start over: the detector keeps the longest tail of what was
/// typed that is still a start of the code, so ↑ ↑ ↑ ↓ ↓ … matches.
#[derive(Clone, Copy, Debug, Default)]
pub struct Konami {
    /// How many keys of [`KONAMI`] have matched.
    matched: usize,
    /// Keys count only while the interact chord is held.
    chord: bool,
}

impl Konami {
    /// A detector for a host with keyboard focus: every key counts.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A detector for a host that sees keys only through the interact
    /// chord: keys count only while the chord is held. Letting go of it
    /// should start over: the host calls [`Konami::reset`] when it sees the
    /// chord released (a key arriving without the chord resets too).
    #[must_use]
    pub fn with_chord() -> Self {
        Self {
            matched: 0,
            chord: true,
        }
    }

    /// Takes one key press; `chord_held` says whether the interact chord is
    /// held (ignored by a [`Konami::new`] detector). Returns `true` when the
    /// press completes the code, and starts over.
    pub fn key(&mut self, key: CheatKey, chord_held: bool) -> bool {
        if self.chord && !chord_held {
            self.matched = 0;
            return false;
        }
        self.matched = Self::advance(self.matched, key);
        if self.matched == KONAMI.len() {
            self.matched = 0;
            return true;
        }
        false
    }

    /// Starts over (the host lost focus, or the chord was let go).
    pub fn reset(&mut self) {
        self.matched = 0;
    }

    /// How many keys of the code have matched so far.
    #[must_use]
    pub fn progress(&self) -> usize {
        self.matched
    }

    /// The longest start of the code that ends the first `matched` keys of
    /// the code followed by `key`.
    fn advance(matched: usize, key: CheatKey) -> usize {
        if KONAMI[matched] == key {
            return matched + 1;
        }
        // Typed so far: KONAMI[..matched] then `key`. Try shorter starts of
        // the code against its tail.
        (1..=matched)
            .rev()
            .find(|&k| KONAMI[k - 1] == key && KONAMI[..k - 1] == KONAMI[matched + 1 - k..matched])
            .unwrap_or(0)
    }
}

/// Longest cheat line a host should let the user type.
pub const MAX_LINE: usize = 64;

/// Most creatures one `spawn` adds.
pub const MAX_SPAWN: usize = 10;

/// The food `feed` sprinkles (SPEC §2.5's Feed with the default food).
pub const FEED_FOOD: &str = "flake_food";

/// How many pinches of food a feed sprinkles across the tank.
pub const FEED_PINCHES: i64 = 12;

/// The species `why not?` adds: a crab-dancing medical crab.
pub const MEDICAL_CRAB: &str = "medical_crab";

/// The species `home owner!` adds: the medical crab in gardening clothes,
/// which plants a carpet patch at a home it picks.
pub const HOME_OWNING_CRAB: &str = "home_owning_crab";

/// Why a cheat line didn't run.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CheatError {
    /// Nothing was typed.
    #[error("type a cheat, or help")]
    Empty,
    /// No cheat has that name.
    #[error("unknown cheat {0:?}; try help")]
    Unknown(String),
    /// The cheat exists but its arguments don't fit.
    #[error("{0}")]
    Usage(String),
    /// The world turned the cheat down.
    #[error("{0}")]
    World(#[from] WorldError),
}

/// What a cheat did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheatOutcome {
    /// A short line for the host to show.
    pub message: String,
    /// Cells changed, so the host can redraw a paused tank.
    pub cells_changed: usize,
    /// Whether the world changed at all (cells, creatures or the water's
    /// fields), so the host knows the tank needs saving.
    pub world_changed: bool,
}

/// One entry in the cheat registry.
#[derive(Clone, Copy, Debug)]
pub struct Cheat {
    /// The word that runs it (lower case), or a phrase of words separated
    /// by single spaces, which must make up the whole line.
    pub name: &'static str,
    /// Its arguments, for `help`.
    pub usage: &'static str,
    /// What it does, for `help`.
    pub help: &'static str,
    run: fn(&mut World, &[&str]) -> Result<CheatOutcome, CheatError>,
}

/// Every cheat, in `help` order.
pub const CHEATS: &[Cheat] = &[
    Cheat {
        name: "help",
        usage: "",
        help: "list the cheats",
        run: help,
    },
    Cheat {
        name: "feed",
        usage: "",
        help: "sprinkle flake food across the tank",
        run: |world, args| {
            no_args("feed", args)?;
            let changed = feed(world);
            Ok(CheatOutcome {
                message: format!("fed: {changed} flakes"),
                cells_changed: changed,
                world_changed: changed > 0,
            })
        },
    },
    Cheat {
        name: "spawn",
        usage: "SPECIES [N]",
        help: "add N (1-10, default 1) creatures of a species",
        run: spawn,
    },
    Cheat {
        name: "clean",
        usage: "",
        help: "change the water and siphon the waste",
        run: |world, args| {
            no_args("clean", args)?;
            let changed = world.water_change();
            let quality = world.ecosystem().quality;
            Ok(CheatOutcome {
                message: format!(
                    "water changed: {changed} cells siphoned, quality {:.0}%",
                    quality * 100.0
                ),
                cells_changed: changed,
                world_changed: true,
            })
        },
    },
    Cheat {
        name: "why not?",
        usage: "",
        help: "add a medical crab",
        run: |world, _| spawn_one(world, MEDICAL_CRAB),
    },
    Cheat {
        name: "home owner!",
        usage: "",
        help: "add a home owning crab, which plants a lawn at its home",
        run: |world, _| spawn_one(world, HOME_OWNING_CRAB),
    },
];

/// Runs one cheat line, such as `spawn neon_tetra 3` or `why not?`. Words
/// are separated by spaces (any run of whitespace, leading and trailing
/// ignored); names and phrases ignore case.
///
/// # Errors
///
/// Returns a [`CheatError`] for an empty line, an unknown cheat, arguments
/// that don't fit, or a world that turns the cheat down; the world is left
/// as it was in each case.
pub fn run(world: &mut World, line: &str) -> Result<CheatOutcome, CheatError> {
    let phrase = line
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if let Some(cheat) = CHEATS
        .iter()
        .find(|c| c.name.contains(' ') && c.name == phrase)
    {
        return (cheat.run)(world, &[]);
    }
    let mut words = line.split_whitespace();
    let name = words.next().ok_or(CheatError::Empty)?.to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    let cheat = CHEATS
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| CheatError::Unknown(truncate(&name, 24)))?;
    (cheat.run)(world, &args)
}

/// Sprinkles [`FEED_FOOD`] just below the water surface in
/// [`FEED_PINCHES`] pinches across the tank (the tray's Feed, SPEC §2.5).
/// Returns how many cells changed: 0 if the world has no such food.
pub fn feed(world: &mut World) -> usize {
    let Some(food) = world.materials().id(FEED_FOOD) else {
        return 0;
    };
    let width = i64::try_from(world.width()).unwrap_or(i64::MAX);
    let row = i64::try_from(world.surface_row()).unwrap_or(0) + 1;
    // Spread evenly, shifted by the tick so feeds don't stack exactly.
    let shift = i64::try_from(world.tick_count() % 7).unwrap_or(0);
    (0..FEED_PINCHES)
        .map(|i| {
            let x = (2 * i + 1) * width / (2 * FEED_PINCHES) + shift - 3;
            pour(world, x, row, food)
        })
        .sum()
}

fn pour(world: &mut World, x: i64, y: i64, material: MaterialId) -> usize {
    world.paint(x, y, 0, material, Brush::Pour).unwrap_or(0)
}

fn help(_: &mut World, args: &[&str]) -> Result<CheatOutcome, CheatError> {
    no_args("help", args)?;
    let list: Vec<String> = CHEATS
        .iter()
        .map(|c| {
            if c.usage.is_empty() {
                c.name.to_owned()
            } else {
                format!("{} {}", c.name, c.usage)
            }
        })
        .collect();
    Ok(CheatOutcome {
        message: format!("cheats: {}", list.join(", ")),
        cells_changed: 0,
        world_changed: false,
    })
}

fn spawn(world: &mut World, args: &[&str]) -> Result<CheatOutcome, CheatError> {
    let usage = || CheatError::Usage(format!("usage: spawn SPECIES [N], N 1-{MAX_SPAWN}"));
    let (name, count) = match args {
        [name] => (*name, 1),
        [name, n] => (*name, n.parse::<usize>().map_err(|_| usage())?),
        _ => return Err(usage()),
    };
    if !(1..=MAX_SPAWN).contains(&count) {
        return Err(usage());
    }
    let name = name.to_ascii_lowercase();
    // Cheat-only species come from their own cheats.
    let Some(species) = world
        .species()
        .iter()
        .find(|s| s.name == name && !s.cheat_only)
        .map(|s| s.id)
    else {
        let names: Vec<&str> = world
            .species()
            .iter()
            .filter(|s| !s.cheat_only)
            .map(|s| s.name.as_str())
            .collect();
        return Err(CheatError::Usage(format!(
            "unknown species {:?}; one of {}",
            truncate(&name, 24),
            names.join(", ")
        )));
    };
    add(world, species, count, &name)
}

/// Adds one creature of the species named `name` (a cheat-only crab).
fn spawn_one(world: &mut World, name: &str) -> Result<CheatOutcome, CheatError> {
    let species = world
        .species()
        .id(name)
        .ok_or_else(|| CheatError::Usage(format!("this tank has no {name}")))?;
    add(world, species, 1, name)
}

/// Adds `count` creatures of `species` (named `name` in the message),
/// spread across the tank in its depth zone, up to the population cap.
fn add(
    world: &mut World,
    species: crate::SpeciesId,
    count: usize,
    name: &str,
) -> Result<CheatOutcome, CheatError> {
    // Its depth zone, or (a glass climber) the whole water column.
    let (top, bottom) = world
        .zone_rows(species)
        .unwrap_or((world.surface_row(), world.height()));
    let y = i64::try_from(usize::midpoint(top, bottom)).unwrap_or(0);
    let width = i64::try_from(world.width()).unwrap_or(i64::MAX);
    let mut added = 0;
    for i in 0..count {
        if world.creatures().len() >= world.creature_cap() {
            break;
        }
        // Spread across the tank, offset by the tick so repeats don't stack.
        let slot = i64::try_from(i).unwrap_or(0);
        let shift = i64::try_from(world.tick_count() % 11).unwrap_or(0);
        let n = i64::try_from(count).unwrap_or(1);
        let x = (2 * slot + 1) * width / (2 * n) + shift - 5;
        match world.add_creature(species, x, y) {
            Ok(_) => added += 1,
            Err(WorldError::NoRoom) => {}
            Err(e) if added == 0 => return Err(e.into()),
            Err(_) => break,
        }
    }
    if added == 0 {
        let cap = world.creature_cap();
        return Err(if world.creatures().len() >= cap {
            WorldError::PopulationCap(cap)
        } else {
            WorldError::NoRoom
        }
        .into());
    }
    Ok(CheatOutcome {
        message: format!("spawned {added} {name}"),
        cells_changed: 0,
        world_changed: true,
    })
}

fn no_args(name: &str, args: &[&str]) -> Result<(), CheatError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(CheatError::Usage(format!("{name} takes no arguments")))
    }
}

/// At most `max` characters of `s`, for echoing user input.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use CheatKey::{A, B, Down, Left, Other, Right, Up};

    fn feed_all(k: &mut Konami, keys: &[CheatKey], chord: bool) -> Vec<bool> {
        keys.iter().map(|&key| k.key(key, chord)).collect()
    }

    #[test]
    fn the_code_fires_once_on_its_last_key() {
        let mut k = Konami::new();
        let fired = feed_all(&mut k, &KONAMI, false);
        assert_eq!(
            fired,
            [
                false, false, false, false, false, false, false, false, false, true
            ]
        );
        assert_eq!(k.progress(), 0, "starts over after firing");
        assert!(feed_all(&mut k, &KONAMI, false)[9], "fires again");
    }

    #[test]
    fn a_wrong_key_keeps_the_longest_matching_tail() {
        let mut k = Konami::new();
        // An extra up: ↑↑↑ still ends in ↑↑.
        let mut keys = vec![Up];
        keys.extend(KONAMI);
        assert_eq!(feed_all(&mut k, &keys, false).last(), Some(&true));
        // A stray key part-way starts over.
        k.reset();
        feed_all(&mut k, &[Up, Up, Down, Other], false);
        assert_eq!(k.progress(), 0);
        // ↑↑↓ then ↑: the tail ↑ is a start of the code.
        feed_all(&mut k, &[Up, Up, Down, Up], false);
        assert_eq!(k.progress(), 1);
        // B A without the arrows does nothing.
        k.reset();
        assert_eq!(feed_all(&mut k, &[B, A], false), [false, false]);
        assert_eq!(k.progress(), 0);
    }

    #[test]
    fn a_chord_detector_counts_keys_only_with_the_chord_held() {
        let mut k = Konami::with_chord();
        assert!(!feed_all(&mut k, &KONAMI, false).contains(&true));
        assert!(feed_all(&mut k, &KONAMI, true)[9]);
        // Letting go part-way starts over.
        feed_all(&mut k, &[Up, Up, Down, Down, Left], true);
        assert!(!k.key(Right, false));
        assert_eq!(k.progress(), 0);
        assert!(!feed_all(&mut k, &KONAMI[5..], true).contains(&true));
        // A focused detector ignores the chord flag.
        let mut k = Konami::new();
        assert!(feed_all(&mut k, &KONAMI, true)[9]);
    }

    fn world() -> World {
        let config = crate::WorldConfig {
            width: 128,
            height: 72,
            seed: 3,
            ..crate::WorldConfig::default()
        };
        World::new(config, crate::MaterialTable::builtin().expect("materials")).expect("world")
    }

    #[test]
    fn unknown_and_malformed_lines_are_errors_and_change_nothing() {
        let mut w = world();
        let hash = w.state_hash();
        assert_eq!(run(&mut w, "   "), Err(CheatError::Empty));
        assert_eq!(
            run(&mut w, "rosebud"),
            Err(CheatError::Unknown("rosebud".to_owned()))
        );
        assert!(matches!(run(&mut w, "feed now"), Err(CheatError::Usage(_))));
        assert!(matches!(run(&mut w, "spawn"), Err(CheatError::Usage(_))));
        assert!(matches!(
            run(&mut w, "spawn neon_tetra 0"),
            Err(CheatError::Usage(_))
        ));
        assert!(matches!(
            run(&mut w, "spawn neon_tetra 11"),
            Err(CheatError::Usage(_))
        ));
        assert!(matches!(
            run(&mut w, "spawn neon_tetra x"),
            Err(CheatError::Usage(_))
        ));
        assert!(matches!(
            run(&mut w, "spawn kraken"),
            Err(CheatError::Usage(_))
        ));
        // A long unknown name is echoed cut short.
        let long = "z".repeat(200);
        assert_eq!(run(&mut w, &long), Err(CheatError::Unknown("z".repeat(24))));
        assert_eq!(w.state_hash(), hash);
        assert_eq!(w.creatures().len(), 0);
    }

    #[test]
    fn help_lists_every_cheat() {
        let out = run(&mut world(), "HELP").expect("help");
        for c in CHEATS {
            assert!(out.message.contains(c.name), "{}", out.message);
        }
    }

    #[test]
    fn feed_sprinkles_flakes_below_the_surface() {
        let mut w = world();
        let out = run(&mut w, "feed").expect("feed");
        let food = w.materials().id(FEED_FOOD).expect("food");
        let flakes = w.material_counts()[usize::from(food.0)];
        assert_eq!(out.cells_changed, flakes);
        assert_eq!(flakes, usize::try_from(FEED_PINCHES).expect("pinches"));
    }

    #[test]
    fn spawn_adds_creatures_of_the_named_species() {
        let mut w = world();
        let out = run(&mut w, "Spawn NEON_TETRA 3").expect("spawn");
        assert_eq!(out.message, "spawned 3 neon_tetra");
        let tetra = w.species().id("neon_tetra").expect("tetra");
        assert_eq!(w.creatures().len(), 3);
        assert!(w.creatures().iter().all(|c| c.species() == tetra));
        // A glass climber, with no depth zone, finds room too.
        run(&mut w, "spawn nerite").expect("nerite");
        assert_eq!(w.creatures().len(), 4);
    }

    /// Spawned creatures are added as from the palette (T9 part 2): sexes
    /// alternate within the species, so `spawn guppy 2` is a pair, and each
    /// settles for a day before it breeds.
    #[test]
    fn spawn_adds_pairs_that_settle_first() {
        let mut w = world();
        run(&mut w, "spawn neon_tetra 3").expect("tetras");
        run(&mut w, "spawn guppy 4").expect("guppies");
        let guppy = w.species().id("guppy").expect("guppy");
        let sexes = |s| {
            let of: Vec<_> = w.creatures().iter().filter(|c| c.species() == s).collect();
            (of.iter().filter(|c| c.female()).count(), of.len())
        };
        assert_eq!(sexes(guppy), (2, 4));
        let tetra = w.species().id("neon_tetra").expect("tetra");
        assert_eq!(sexes(tetra), (1, 3));
        assert!(
            w.creatures()
                .iter()
                .all(|c| c.rest() == crate::SETTLE_TICKS && c.carrying() == 0)
        );
    }

    #[test]
    fn spawn_stops_at_the_population_cap() {
        let mut w = world();
        let mut added = 0;
        while let Ok(out) = run(&mut w, "spawn neon_tetra 10") {
            assert!(out.message.starts_with("spawned"));
            added += 1;
            assert!(added <= w.creature_cap(), "never refused");
        }
        let cap = w.creature_cap();
        assert!(w.creatures().len() <= cap);
        if w.creatures().len() == cap {
            assert_eq!(
                run(&mut w, "spawn neon_tetra"),
                Err(CheatError::World(WorldError::PopulationCap(cap)))
            );
        }
    }

    #[test]
    fn clean_changes_the_water() {
        let mut w = world();
        let out = run(&mut w, "clean").expect("clean");
        assert!(out.message.starts_with("water changed"), "{}", out.message);
    }
}
