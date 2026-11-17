//! Writes PPM snapshots of a few test scenes to `target/shots/sim/`, for
//! looking at the movement rules before the renderer exists.
//!
//! `cargo run --release -p desktank-sim --example snapshot`

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use desktank_sim::{Brush, MaterialTable, Rect, World, WorldConfig, snapshot};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new("target/shots/sim");
    fs::create_dir_all(dir)?;
    let t = MaterialTable::builtin()?;
    let id = |n: &str| t.id(n).ok_or_else(|| format!("no material {n}"));
    let (sand, gravel, rock, bubble, flake) = (
        id("sand")?,
        id("gravel")?,
        id("rock")?,
        id("air_bubble")?,
        id("flake_food")?,
    );

    let mut w = World::new(
        WorldConfig {
            width: 320,
            height: 180,
            seed: 1,
            water_level: 0.85,
        },
        t.clone(),
    )?;
    w.fill_rect(
        Rect {
            x0: 230,
            y0: 150,
            x1: 290,
            y1: 180,
        },
        rock,
        Brush::Replace,
    )?;
    let mut frame = 0;
    for tick in 0..2400_u32 {
        if tick < 1200 {
            w.paint(100, 2, 1, sand, Brush::Pour)?;
            w.paint(180, 2, 0, gravel, Brush::Pour)?;
        }
        if tick % 10 == 0 {
            w.paint(260, 140, 0, bubble, Brush::Pour)?;
        }
        if tick % 200 == 0 && tick < 1000 {
            w.paint(40, 5, 0, flake, Brush::Pour)?;
        }
        w.tick();
        if tick % 600 == 599 {
            save(&w, &dir.join(format!("scene_{frame}.ppm")))?;
            frame += 1;
        }
    }
    println!("mixed scene: awake chunks at end: {}", w.awake_chunks());

    // The heavy-pour bench scene, once settled.
    let mut w = World::new(WorldConfig::default(), t)?;
    let column = Rect {
        x0: 240,
        y0: 0,
        x1: 400,
        y1: 288,
    };
    w.fill_rect(column, sand, Brush::Replace)?;
    save(&w, &dir.join("heavy_pour_start.ppm"))?;
    while {
        w.tick();
        w.awake_chunks() > 0
    } {}
    println!("heavy pour settled after {} ticks", w.tick_count());
    save(&w, &dir.join("heavy_pour_settled.ppm"))?;
    Ok(())
}

fn save(w: &World, path: &Path) -> std::io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    snapshot::write_ppm(w, 2, &mut out)
}
