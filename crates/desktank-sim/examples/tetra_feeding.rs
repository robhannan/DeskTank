//! Repeated feeding of a school of six neon tetras, one flake at a time.
//!
//! Each seeded school runs continuously, so every fish carries its feeding
//! history from one flake to the next. One flake drops on alternating sides
//! every `interval` seconds, plus a matching run with no food at all.
//!
//! Only APIs shared with the pre-needs build (`81a44bd`) are used, so the same
//! file builds there and in later commits for a like-for-like comparison.
//! See `docs/notes/tetra-feeding-pilot.md` for the recipe and results.
//!
//! ```text
//! cargo run --release -p desktank-sim --example tetra_feeding            # summary table
//! cargo run --release -p desktank-sim --example tetra_feeding -- --csv   # one row per flake
//! cargo run --release -p desktank-sim --example tetra_feeding -- --trace SEED   # replay positions
//! ```

use std::error::Error;
use std::io::{self, BufWriter, Write};

use desktank_sim::{
    Brush, Creature, MaterialId, MaterialTable, TICKS_PER_SECOND, World, WorldConfig,
};

const WIDTH: usize = 160;
const HEIGHT: usize = 90;
const WATER_LEVEL: f32 = 0.96;
const FISH: usize = 6;
/// Seeded schools in a summary, unless `--seeds N` says otherwise.
const SEEDS: u64 = 12;
const DROPS: u32 = 12;
/// Quiet seconds before the first flake, so the school settles.
const WARMUP: u32 = 12 * TICKS_PER_SECOND;
const DROP_Y: i64 = 2;
const LEFT: i64 = 54;
const RIGHT: i64 = 106;
/// A fish "approaches" when it ends up this many cells closer to the drop
/// within two seconds.
const APPROACH_CELLS: f32 = 10.0;
const EARLY_WINDOW: u32 = 2 * TICKS_PER_SECOND;
const BITE_WINDOW: u32 = 10 * TICKS_PER_SECOND;
/// Feeding intervals in seconds. The last run has no food at all.
const INTERVALS: [(u32, bool); 3] = [(15, true), (90, true), (15, false)];

fn spawn() -> impl Iterator<Item = (i64, i64)> {
    (0..i64::try_from(FISH).unwrap()).map(|i| (64 + 6 * i, 39 + 5 * (i % 3)))
}

fn drop_x(index: u32) -> i64 {
    if index.is_multiple_of(2) { LEFT } else { RIGHT }
}

#[allow(clippy::cast_precision_loss)] // cell coordinates are small
fn drop_distance(fish: &Creature, x: i64) -> f32 {
    let (fx, fy) = fish.centre();
    (fx - x as f32).hypot(fy - DROP_Y as f32)
}

fn school(seed: u64) -> Result<World, Box<dyn Error>> {
    let mut world = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level: WATER_LEVEL,
        },
        MaterialTable::builtin()?,
    )?;
    let tetra = world.species().id("neon_tetra").ok_or("missing tetra")?;
    for (x, y) in spawn() {
        world.add_creature(tetra, x, y)?;
    }
    for _ in 0..WARMUP {
        world.tick();
    }
    Ok(world)
}

fn flake_cells(world: &World, flake: MaterialId) -> Vec<usize> {
    world
        .cells()
        .iter()
        .enumerate()
        .filter(|(_, cell)| cell.material == flake)
        .map(|(i, _)| i)
        .collect()
}

/// One flake presentation.
#[derive(Clone, Copy, Default)]
struct Drop {
    /// Fish that ended up at least 10 cells nearer the drop within 2 s.
    approaches: usize,
    /// The first fish to bite, and how many ticks that took (within 10 s).
    first: Option<(u32, u32)>,
    /// Flake cells still in the tank 10 s after the drop.
    left_at_ten: usize,
    /// Which fish moved closer within 2 s, in spawn order.
    approached: [bool; FISH],
    /// Which fish bit at any point before the next flake.
    fed: [bool; FISH],
}

/// Seconds-per-fish of motion that changes direction, and mean nearest-mate gap.
#[derive(Clone, Copy, Default)]
struct Motion {
    flips: u64,
    fish_ticks: u64,
    gap_sum: f64,
    gap_ticks: u64,
}

impl Motion {
    fn record(&mut self, world: &World, last_facing: &mut [bool]) {
        for (i, fish) in world.creatures().iter().take(FISH).enumerate() {
            self.flips += u64::from(last_facing[i] != fish.facing_left());
            last_facing[i] = fish.facing_left();
            self.fish_ticks += 1;
            let (fx, fy) = fish.centre();
            let nearest = world
                .creatures()
                .iter()
                .take(FISH)
                .filter(|m| m.id() != fish.id())
                .map(|m| {
                    let (mx, my) = m.centre();
                    (fx - mx).hypot(fy - my)
                })
                .fold(f32::MAX, f32::min);
            self.gap_sum += f64::from(nearest);
            self.gap_ticks += 1;
        }
    }
}

struct Run {
    drops: Vec<Drop>,
    motion: Motion,
    /// First-bite fish count by fish, in spawn order.
    firsts: [u32; FISH],
}

fn run(seed: u64, interval: u32, feeding: bool) -> Result<Run, Box<dyn Error>> {
    let mut world = school(seed)?;
    let flake = world.materials().id("flake_food").ok_or("missing flake")?;
    let water = world.materials().id("water").ok_or("missing water")?;
    let ids: Vec<_> = world
        .creatures()
        .iter()
        .take(FISH)
        .map(Creature::id)
        .collect();
    let mut facing: Vec<bool> = world
        .creatures()
        .iter()
        .take(FISH)
        .map(Creature::facing_left)
        .collect();
    let mut out = Run {
        drops: Vec::new(),
        motion: Motion::default(),
        firsts: [0; FISH],
    };
    for drop in 0..DROPS {
        let x = drop_x(drop);
        if feeding {
            world.paint(x, DROP_Y, 0, flake, Brush::Pour)?;
        }
        let before: Vec<(f32, u32)> = ids
            .iter()
            .map(|&id| {
                let fish = world.creature(id).expect("tank keeps its fish");
                (drop_distance(fish, x), fish.eaten())
            })
            .collect();
        let mut result = Drop::default();
        for t in 1..=interval * TICKS_PER_SECOND {
            world.tick();
            out.motion.record(&world, &mut facing);
            for (i, &id) in ids.iter().enumerate() {
                let fish = world.creature(id).expect("tank keeps its fish");
                if t <= EARLY_WINDOW && before[i].0 - drop_distance(fish, x) >= APPROACH_CELLS {
                    result.approached[i] = true;
                }
                if feeding && fish.eaten() > before[i].1 {
                    result.fed[i] = true;
                }
                if feeding
                    && t <= BITE_WINDOW
                    && fish.eaten() > before[i].1
                    && result.first.is_none()
                {
                    result.first = Some((u32::try_from(i).unwrap_or(0), t));
                }
            }
            if t == BITE_WINDOW {
                result.left_at_ten = flake_cells(&world, flake).len();
            }
        }
        result.approaches = result.approached.into_iter().filter(|&a| a).count();
        if let Some((i, _)) = result.first {
            out.firsts[usize::try_from(i).unwrap_or(0)] += 1;
        }
        out.drops.push(result);
        // Each presentation is one flake: clear any that was missed.
        if feeding {
            for i in flake_cells(&world, flake) {
                world.set_cell(i % WIDTH, i / WIDTH, water)?;
            }
        }
    }
    Ok(out)
}

fn csv(out: &mut impl Write, seeds: u64) -> Result<(), Box<dyn Error>> {
    writeln!(
        out,
        "seed,interval_s,feeding,drop,x,approaches,first_fish,first_ticks,fed,flakes_at_10s"
    )?;
    for (interval, feeding) in INTERVALS {
        for seed in 1..=seeds {
            for (i, d) in run(seed, interval, feeding)?.drops.iter().enumerate() {
                let (fish, ticks) = d.first.map_or((-1, 0), |(f, t)| (i64::from(f), t));
                writeln!(
                    out,
                    "{seed},{interval},{feeding},{},{},{},{fish},{ticks},{},{}",
                    i + 1,
                    drop_x(u32::try_from(i)?),
                    d.approaches,
                    d.fed.into_iter().filter(|&f| f).count(),
                    d.left_at_ten
                )?;
            }
        }
    }
    Ok(())
}

/// Counts over every flake of every school at one interval.
#[derive(Default)]
struct Tally {
    flakes: usize,
    approaches: usize,
    eaten: usize,
    bite_ticks: u64,
    left_at_ten: usize,
    nobody_approached: usize,
    /// Flakes whose first eater also bit the flake before.
    repeats: usize,
    pairs: usize,
    /// The busiest first eater's share of each school's flakes, summed.
    busiest: f64,
    distinct: usize,
    /// Flakes whose first eater bit one of the last two flakes, and the
    /// share of the school that did, summed.
    lately_first: usize,
    lately_share: f64,
    lately_n: usize,
    /// Fish that bit the last flake, or did not, and how many approached.
    after_meal: (usize, usize),
    otherwise: (usize, usize),
    flips: u64,
    fish_ticks: u64,
    gap_sum: f64,
    gap_ticks: u64,
}

#[allow(clippy::cast_precision_loss)] // small counts
fn pct(part: usize, whole: usize) -> f64 {
    100.0 * part as f64 / whole.max(1) as f64
}

impl Tally {
    fn add(&mut self, run: &Run, feeding: bool) {
        for (i, d) in run.drops.iter().enumerate() {
            self.flakes += 1;
            self.approaches += d.approaches;
            self.left_at_ten += d.left_at_ten;
            self.nobody_approached += usize::from(d.approaches == 0);
            if let Some((_, t)) = d.first {
                self.eaten += 1;
                self.bite_ticks += u64::from(t);
            }
            if i > 0
                && let (Some((a, _)), Some((b, _))) = (run.drops[i - 1].first, d.first)
            {
                self.pairs += 1;
                self.repeats += usize::from(a == b);
            }
            if i > 1
                && let Some((first, _)) = d.first
            {
                let lately = |f: usize| run.drops[i - 1].fed[f] || run.drops[i - 2].fed[f];
                self.lately_first += usize::from(lately(usize::try_from(first).unwrap_or(0)));
                self.lately_share += pct((0..FISH).filter(|&f| lately(f)).count(), FISH);
                self.lately_n += 1;
            }
            if i > 0 && feeding {
                for f in 0..FISH {
                    let tally = if run.drops[i - 1].fed[f] {
                        &mut self.after_meal
                    } else {
                        &mut self.otherwise
                    };
                    tally.0 += usize::from(d.approached[f]);
                    tally.1 += 1;
                }
            }
        }
        self.busiest += pct(
            usize::try_from(*run.firsts.iter().max().unwrap_or(&0)).unwrap_or(0),
            run.drops.len(),
        );
        self.distinct += run.firsts.iter().filter(|&&n| n > 0).count();
        self.flips += run.motion.flips;
        self.fish_ticks += run.motion.fish_ticks;
        self.gap_sum += run.motion.gap_sum;
        self.gap_ticks += run.motion.gap_ticks;
    }

    #[allow(clippy::cast_precision_loss)] // small counts
    fn row(&self, label: &str, feeding: bool, seeds: u64) -> String {
        let (n, seeds) = (self.flakes, seeds as f64);
        let per = |x: usize| x as f64 / n.max(1) as f64;
        let seconds = self.fish_ticks as f64 / f64::from(TICKS_PER_SECOND);
        let motion = format!(
            "{:.1} | {:.1}",
            60.0 * self.flips as f64 / seconds,
            self.gap_sum / self.gap_ticks as f64
        );
        if !feeding {
            return format!(
                "| {label} | {:.2} | - | - | - | - | - | - | - | {}/{n} | - | - | {motion} |",
                per(self.approaches),
                self.nobody_approached
            );
        }
        let bite_s =
            self.bite_ticks as f64 / f64::from(TICKS_PER_SECOND) / self.eaten.max(1) as f64;
        format!(
            "| {label} | {:.2} | {}/{n} | {bite_s:.2} | {:.2} | {:.0}% ({}/{}) | {:.0}% | {:.2} of 6 | {:.0}% ({:.0}%) | {}/{n} | {:.0}% ({}) | {:.0}% ({}) | {motion} |",
            per(self.approaches),
            self.eaten,
            per(self.left_at_ten),
            pct(self.repeats, self.pairs),
            self.repeats,
            self.pairs,
            self.busiest / seeds,
            self.distinct as f64 / seeds,
            pct(self.lately_first, self.lately_n),
            self.lately_share / self.lately_n.max(1) as f64,
            self.nobody_approached,
            pct(self.after_meal.0, self.after_meal.1),
            self.after_meal.1,
            pct(self.otherwise.0, self.otherwise.1),
            self.otherwise.1,
        )
    }
}

fn summary(out: &mut impl Write, seeds: u64) -> Result<(), Box<dyn Error>> {
    writeln!(
        out,
        "| interval | early approaches/flake | flakes eaten in 10 s | first bite (s) | \
         flakes left at 10 s | repeat first eater | busiest fish share | distinct first eaters | \
         first eater fed in last 2 flakes (expected) | flakes nobody approached | \
         approach after a meal | approach otherwise | turns/fish/min | nearest mate (cells) |"
    )?;
    writeln!(out, "|{}", "---|".repeat(14))?;
    for (interval, feeding) in INTERVALS {
        let mut tally = Tally::default();
        for seed in 1..=seeds {
            tally.add(&run(seed, interval, feeding)?, feeding);
        }
        let label = if feeding {
            format!("{interval} s")
        } else {
            "no food".to_string()
        };
        writeln!(out, "{}", tally.row(&label, feeding, seeds))?;
    }
    Ok(())
}

/// Positions of the fish and flake every second tick, for a replay: four
/// quiet seconds, a flake on the left, another on the right 15 s later, then
/// a long quiet spell. `P,tick,id,x,y,left,hungry,eaten` and `F,tick,x,y`.
fn trace(out: &mut impl Write, seed: u64) -> Result<(), Box<dyn Error>> {
    let mut world = school(seed)?;
    let flake = world.materials().id("flake_food").ok_or("missing flake")?;
    let total = 46 * TICKS_PER_SECOND;
    let drops = [0, 15 * TICKS_PER_SECOND];
    writeln!(
        out,
        "# seed {seed}, {WIDTH}x{HEIGHT}, flakes at t=0 and t=15 s, drop x {LEFT} then {RIGHT}"
    )?;
    for t in 0..total {
        if let Some(n) = drops.iter().position(|&d| d == t) {
            world.paint(drop_x(u32::try_from(n)?), DROP_Y, 0, flake, Brush::Pour)?;
        }
        world.tick();
        if t % 2 == 1 {
            for fish in world.creatures().iter().take(FISH) {
                let (x, y) = fish.centre();
                writeln!(
                    out,
                    "P,{t},{},{x:.2},{y:.2},{},{},{}",
                    fish.id().0,
                    u8::from(fish.facing_left()),
                    u8::from(fish.hungry()),
                    fish.eaten()
                )?;
            }
            for i in flake_cells(&world, flake) {
                writeln!(out, "F,{t},{},{}", i % WIDTH, i / WIDTH)?;
            }
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = BufWriter::new(io::stdout().lock());
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--csv") => csv(&mut out, SEEDS)?,
        Some("--trace") => trace(&mut out, args.get(1).map_or(Ok(1), |s| s.parse())?)?,
        Some("--seeds") => summary(&mut out, args.get(1).map_or(Ok(SEEDS), |s| s.parse())?)?,
        _ => summary(&mut out, SEEDS)?,
    }
    out.flush()?;
    Ok(())
}
