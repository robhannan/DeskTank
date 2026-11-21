//! Smoke test: draw a tiny world offscreen and read pixels back. Skipped
//! (with a message) on machines without a GPU adapter, but fails when `CI`
//! is set: CI has Metal, WARP/DX12 and lavapipe. Reference-image tests are
//! in `crates/desktank/tests/render.rs`.

use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, wgpu};
use desktank_sim::{Brush, MaterialTable, World, WorldConfig};

const W: u32 = 80;
const H: u32 = 32;

fn gpu() -> Option<Gpu> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    match pollster::block_on(Gpu::new(&instance, None, GpuOptions::default())) {
        Ok(g) => Some(g),
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping: {e}");
            None
        }
        Err(e) => panic!("no GPU in CI: {e}"),
    }
}

/// Renders `tank` into a `W`×`H` RGBA target of `format` and returns tightly packed rows.
fn render(gpu: &Gpu, tank: &mut Tank, format: wgpu::TextureFormat) -> Vec<u8> {
    render_sized(gpu, tank, format, (W, H))
}

/// [`render`] into a `w`×`h` target.
fn render_sized(
    gpu: &Gpu,
    tank: &mut Tank,
    format: wgpu::TextureFormat,
    (w, h): (u32, u32),
) -> Vec<u8> {
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    tank.render_to(gpu, &view, format, (w, h));
    let padded = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        size,
    );
    gpu.queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let data = buffer.get_mapped_range(..).unwrap();
    data.chunks(padded as usize)
        .flat_map(|row| &row[..(w * 4) as usize])
        .copied()
        .collect()
}

fn px(img: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * W + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

#[test]
fn draws_cells_scaled_and_letterboxed() {
    let Some(gpu) = gpu() else { return };
    let materials = MaterialTable::builtin().unwrap();
    let rock = materials.id("rock").unwrap();
    let rock_colour = materials.get(rock).unwrap().colour;
    let config = WorldConfig {
        width: 16,
        height: 8,
        seed: 1,
        water_level: 1.0,
    };
    let mut world = World::new(config, materials).unwrap();
    world.set_cell(3, 5, rock).unwrap();
    let mut tank = Tank::new(world, TankConfig::default());
    let img = render(&gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);

    // 16×8 cells in 80×32 px: scale 4, centred with an 8 px letterbox.
    let backdrop = px(&img, 2, 10);
    for (got, want) in backdrop.iter().zip([5u8, 10, 18]) {
        assert!(got.abs_diff(want) <= 1, "backdrop {backdrop:?}");
    }
    // The rock cell covers pixels 20..24 × 20..24, shaded at 4×4 sub-cell
    // resolution (top edge lit, bottom edge shadowed, every pixel near rock):
    // a cell set on its own is whole, so it stays square.
    for y in 20..24 {
        for x in 20..24 {
            let p = px(&img, x, y);
            for (got, base) in p.iter().zip(rock_colour) {
                assert!(
                    got.abs_diff(base) <= 32,
                    "rock ({x},{y}) {p:?} vs {rock_colour:?}"
                );
            }
        }
    }
    assert!(
        px(&img, 21, 20)[0] > px(&img, 21, 23)[0],
        "rock top bevel should be brighter than bottom edge"
    );
    // Water around it is blue.
    let [wr, _, wb] = px(&img, 9, 1);
    assert!(wb > wr);
}

/// A radius-10 rock pour draws a round disc at 4×, not the octagon its
/// cells make: each texel row's width follows the brush's circle within a
/// texel on each side, its top rim is lit and its underside shadowed.
#[test]
fn a_poured_rock_disc_is_round() {
    let Some(gpu) = gpu() else { return };
    let materials = MaterialTable::builtin().unwrap();
    let rock = materials.id("rock").unwrap();
    let config = WorldConfig {
        width: 24,
        height: 24,
        seed: 1,
        water_level: 1.0,
    };
    let mut world = World::new(config, materials).unwrap();
    world.paint(12, 12, 10, rock, Brush::Replace).unwrap();
    let mut tank = Tank::new(world, TankConfig::default());
    // 24×24 cells in 96×96 px: scale 4, no letterbox.
    let side = 96;
    let img = render_sized(
        &gpu,
        &mut tank,
        wgpu::TextureFormat::Rgba8Unorm,
        (side, side),
    );
    let at = |x: u32, y: u32| {
        let i = ((y * side + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    let is_rock = |p: [u8; 3]| p[2] < p[0].saturating_add(16);
    // The brush's circle: radius √(r² + r/4) cells about cell (12, 12)'s
    // centre, in texels.
    let radius = 4.0 * (100.0_f64 + 2.5).sqrt();
    let centre = 4.0 * 12.0 + 2.0;
    let mut rows = Vec::new();
    for y in 0..side {
        let cols: Vec<u32> = (0..side).filter(|&x| is_rock(at(x, y))).collect();
        let dy = f64::from(y) + 0.5 - centre;
        let half = (radius * radius - dy * dy).max(0.0).sqrt();
        let ideal = 2.0 * half;
        let Some((&first, &last)) = cols.first().zip(cols.last()) else {
            assert!(ideal < 2.5, "row {y}: no rock, circle {ideal:.1} wide");
            continue;
        };
        assert_eq!(
            usize::try_from(last - first + 1).unwrap(),
            cols.len(),
            "row {y} has a gap"
        );
        let width = f64::from(last - first + 1);
        assert!(
            (width - ideal).abs() <= 2.5,
            "row {y}: {width} texels wide, circle {ideal:.1}"
        );
        rows.push((y, first, last));
    }
    // Rounder than the octagon: its flat top spans 7 cells (28 texels);
    // the disc's top row is a short arc.
    let (top, first, last) = rows[0];
    assert!(last - first < 20, "top row {top}: {first}..={last}");
    // Lit on top, shadowed underneath (centre column).
    let (bottom, ..) = *rows.last().unwrap();
    let mid = side / 2;
    let lum = |p: [u8; 3]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    assert!(
        lum(at(mid, top)) > lum(at(mid, bottom)),
        "top rim {:?} vs underside {:?}",
        at(mid, top),
        at(mid, bottom)
    );
}

#[test]
fn scale_one_keeps_flat_cells_and_depth_tints_water() {
    let Some(gpu) = gpu() else { return };
    let materials = MaterialTable::builtin().unwrap();
    let rock = materials.id("rock").unwrap();
    let rock_colour = materials.get(rock).unwrap().colour;
    let config = WorldConfig {
        width: 16,
        height: 8,
        seed: 1,
        water_level: 0.75,
    };
    let mut world = World::new(config, materials).unwrap();
    world.set_cell(3, 5, rock).unwrap();
    let mut tank = Tank::new(world, TankConfig::default());
    let img = tank.capture(&gpu, (16, 8)).unwrap();
    let at = |x: u32, y: u32| -> [u8; 3] {
        let i = ((y * 16 + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    // Rock stays near its flat palette shade.
    let r = at(3, 5);
    for (got, base) in r.iter().zip(rock_colour) {
        assert!(got.abs_diff(base) <= 20, "rock {r:?} vs {rock_colour:?}");
    }
    // Air at row 0 is lighter than water, and deep water darkens with depth.
    let air = at(10, 0);
    let top_water = at(10, 2);
    let deep_water = at(10, 7);
    assert!(air[0] > top_water[0] && air[2] > top_water[2]);
    assert!(top_water[2] > deep_water[2]);
}

#[test]
fn srgb_targets_show_the_same_colours() {
    let Some(gpu) = gpu() else { return };
    let materials = MaterialTable::builtin().unwrap();
    let config = WorldConfig {
        width: 16,
        height: 8,
        seed: 1,
        water_level: 1.0,
    };
    let world = World::new(config, materials).unwrap();
    let mut tank = Tank::new(world, TankConfig::default());
    let linear = render(&gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
    let srgb = render(&gpu, &mut tank, wgpu::TextureFormat::Rgba8UnormSrgb);
    // Backdrop and a water cell match within rounding.
    for (x, y) in [(2, 10), (9, 1)] {
        let (a, b) = (px(&linear, x, y), px(&srgb, x, y));
        for (p, q) in a.iter().zip(b) {
            assert!(p.abs_diff(q) <= 2, "({x}, {y}): {a:?} vs {b:?}");
        }
    }
}

#[test]
fn a_starving_plant_yellows_without_rewriting_its_cells() {
    let Some(gpu) = gpu() else { return };
    let materials = MaterialTable::builtin().unwrap();
    let leaf = materials.id("leaf").unwrap();
    let config = WorldConfig {
        width: 16,
        height: 8,
        seed: 1,
        water_level: 1.0,
    };
    let mut world = World::new(config, materials).unwrap();
    // One small plant (a leaf patch, the default species), in water with
    // no nutrients at all: it starves.
    world
        .fill_rect(
            desktank_sim::Rect {
                x0: 3,
                y0: 4,
                x1: 6,
                y1: 6,
            },
            leaf,
            desktank_sim::Brush::Replace,
        )
        .unwrap();
    let mut tank = Tank::new(world, TankConfig::default());
    tank.step();
    let healthy = render(&gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
    // The cell at (4, 5) covers pixels 24..28 × 20..24.
    let before = px(&healthy, 25, 21);
    let table = tank.world().plant_species();
    let plant = table.get(table.id("hygrophila").unwrap()).unwrap();
    let ramp = &plant.colours.leaf;
    // The cell's depth cue (tank.wgsl `depth_cue`: faded towards the haze
    // and dimmed, by band).
    let t = f32::from(tank.world().plant_band(4, 5).unwrap()) / 7.0;
    let cue = |c: u8, haze: f32| {
        let c = f32::from(c) / 255.0;
        let v = (c + (haze - c) * 0.22 * t) * (1.0 - 0.1 * t);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = (v * 255.0).round() as u8;
        v
    };
    // A shade of the species' leaf ramp (resampled to the texture's eight
    // shades, so within half a ramp step of an entry).
    let nearest = ramp
        .iter()
        .map(|shade| {
            shade
                .iter()
                .zip([0.11, 0.27, 0.42])
                .map(|(&c, haze)| cue(c, haze))
                .zip(before)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap()
        })
        .min()
        .unwrap();
    assert!(
        nearest <= 24,
        "healthy leaf colour {before:?} is not a shade of its species ramp {ramp:?}",
    );
    // A minute later its health has fallen (not yet to the 0.5 at which
    // leaves drop); no cell changed in the last ticks, yet the tank wants
    // a frame and draws it yellower.
    for _ in 0..60 * 20 {
        tank.step();
    }
    let p = tank.world().plants().next().unwrap();
    assert!(p.health < 0.8 && p.health > 0.5, "{p:?}");
    assert_eq!(tank.world().dirty_rects().count(), 0);
    assert!(tank.frame_wanted());
    let starving = render(&gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
    let after = px(&starving, 25, 21);
    assert!(after[2] + 8 < before[2], "{before:?} -> {after:?}");
    assert!(after[0] >= before[0], "{before:?} -> {after:?}");
}

/// D21: a creature behind a plant cell (in a band nearer the glass) is
/// hidden by it; one in front of plant cells is drawn over them. At scale 1
/// every texel of a plant cell is covered, so the check is per cell.
#[test]
fn plants_in_front_hide_creatures_behind_them() {
    let Some(gpu) = gpu() else { return };
    let (mut hidden, mut shown) = (0, 0);
    for seed in 1..=24 {
        let materials = MaterialTable::builtin().unwrap();
        let leaf = materials.id("leaf").unwrap();
        // 80×32 cells in an 80×32 target: one pixel per cell.
        let config = WorldConfig {
            width: W as usize,
            height: H as usize,
            seed,
            water_level: 1.0,
        };
        let mut world = World::new(config, materials).unwrap();
        world
            .fill_rect(
                desktank_sim::Rect {
                    x0: 20,
                    y0: 6,
                    x1: 60,
                    y1: 26,
                },
                leaf,
                desktank_sim::Brush::Replace,
            )
            .unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        // Placed in a band clear of the leaves it overlaps, or beside them.
        let id = world.add_creature(tetra, 40, 16).unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let with = render(&gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
        let world = tank.world_mut();
        let depth = world.creature(id).unwrap().depth();
        let (cx, cy) = world.creature(id).unwrap().centre();
        assert!(world.remove_creature(id));
        tank.invalidate();
        let without = render(&gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
        let world = tank.world();
        let mut covered = 0;
        for y in 0..H {
            for x in 0..W {
                let band = world.plant_band(x as usize, y as usize);
                let in_front = band.is_some_and(|b| f32::from(b) + 0.5 < depth);
                let changed = px(&with, x, y) != px(&without, x, y);
                assert!(
                    !(in_front && changed),
                    "seed {seed}: creature at depth {depth} drawn over a band {band:?} cell at ({x}, {y})"
                );
                if changed && band.is_some() {
                    covered += 1;
                }
            }
        }
        // A creature inside the leaf block is either hidden by them (no
        // leaf pixel changed) or drawn over them.
        let inside = (26.0..54.0).contains(&cx) && (10.0..22.0).contains(&cy);
        if inside && covered == 0 {
            hidden += 1;
        } else if covered > 0 {
            shown += 1;
        }
    }
    assert!(hidden > 0 && shown > 0, "hidden {hidden}, shown {shown}");
}

#[test]
fn flow_tint_shows_speed_in_either_direction() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    for outlet_x in [5, 74] {
        let config = WorldConfig {
            width: W as usize,
            height: H as usize,
            seed: 4,
            water_level: 1.0,
        };
        let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        world
            .place(desktank_sim::Placeable::FilterOutlet, outlet_x, 20)
            .unwrap();
        world.tick();
        let mut tank = Tank::new(world, TankConfig::default());
        let plain = render(&gpu, &mut tank, format);
        tank.set_field_overlay(Some(desktank_render::FieldOverlay::Flow));
        let tinted = render(&gpu, &mut tank, format);
        let x = if outlet_x == 5 { 20 } else { 59 };
        let gain = |y| i16::from(px(&tinted, x, y)[0]) - i16::from(px(&plain, x, y)[0]);
        assert!(gain(20) > gain(16) && gain(16) > 0);
        assert_eq!(px(&plain, x, 0), px(&tinted, x, 0));
        tank.set_field_overlay(None);
        assert_eq!(plain, render(&gpu, &mut tank, format));
    }
}

fn gpu_with(options: GpuOptions) -> Option<Gpu> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    match pollster::block_on(Gpu::new(&instance, None, options)) {
        Ok(g) => Some(g),
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping: {e}");
            None
        }
        Err(e) => panic!("no GPU in CI: {e}"),
    }
}

/// The magnifier zooms a single rock cell and a creature sprite inside the
/// lens rect, draws a border around it, leaves pixels outside untouched, and
/// works identically on both single-pass and split-grid (`webOS`) pipelines.
#[test]
fn magnifier_zooms_cells_and_sprites_in_single_and_split_grid() {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (tw, th) = (320u32, 180u32);
    let px_at = |img: &[u8], x: u32, y: u32| -> [u8; 3] {
        let i = ((y * tw + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    let mut single_2x: Option<Vec<u8>> = None;
    for split_grid in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid,
            ..GpuOptions::default()
        }) else {
            return;
        };
        let materials = MaterialTable::builtin().unwrap();
        let rock = materials.id("rock").unwrap();
        let rock_colour = materials.get(rock).unwrap().colour;
        let config = WorldConfig {
            width: 80,
            height: 45,
            seed: 7,
            water_level: 1.0,
        };
        let mut world = World::new(config, materials).unwrap();
        // Single rock cell at (40, 22) and a creature beside it at (43, 22).
        world.set_cell(40, 22, rock).unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        world.add_creature(tetra, 43, 22).unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let plain = render_sized(&gpu, &mut tank, format, (tw, th));

        let layout = tank.layout((tw, th));
        assert_eq!(layout.scale, 4);

        for zoom in [2u32, 4u32] {
            let mag = desktank_render::Magnifier {
                cell: (40, 22),
                zoom,
            };
            let ml = desktank_render::MagnifierLayout::fit((tw, th), &layout, mag).unwrap();
            tank.set_magnifier(Some(mag));
            let zoomed = render_sized(&gpu, &mut tank, format, (tw, th));

            // Pixels outside the lens rect are byte-identical to the plain frame.
            for y in 0..th {
                for x in 0..tw {
                    let in_lens = x >= ml.origin.0
                        && x < ml.origin.0 + ml.size.0
                        && y >= ml.origin.1
                        && y < ml.origin.1 + ml.size.1;
                    if !in_lens {
                        assert_eq!(
                            px_at(&zoomed, x, y),
                            px_at(&plain, x, y),
                            "split={split_grid} zoom={zoom}: outside lens ({x},{y}) changed"
                        );
                    }
                }
            }

            // Inside the lens, cell (40, 22) covers an exact (4 * zoom)^2 block
            // centred at the lens centre.
            let cell_px = layout.scale * zoom;
            let cx = ml.origin.0 + ml.size.0 / 2;
            let cy = ml.origin.1 + ml.size.1 / 2;
            let x0 = cx - cell_px / 2;
            let y0 = cy - cell_px / 2;
            for y in y0..y0 + cell_px {
                for x in x0..x0 + cell_px {
                    let p = px_at(&zoomed, x, y);
                    for (got, base) in p.iter().zip(rock_colour) {
                        assert!(
                            got.abs_diff(base) <= 40,
                            "split={split_grid} zoom={zoom}: magnified rock at ({x},{y}) {p:?} vs {rock_colour:?}"
                        );
                    }
                }
            }

            // The lens border is drawn along the outer edge of the lens rect.
            let border = px_at(&zoomed, ml.origin.0, ml.origin.1);
            let water = px_at(&plain, ml.origin.0, ml.origin.1);
            assert_ne!(
                border, water,
                "split={split_grid} zoom={zoom}: expected visible lens border"
            );

            if zoom == 2 {
                if !split_grid {
                    single_2x = Some(zoomed);
                } else if let Some(ref single) = single_2x {
                    assert_eq!(
                        zoomed, *single,
                        "split-grid magnified output must match single-pass"
                    );
                }
            }
        }

        // Turning the magnifier off restores the plain frame identically.
        tank.set_magnifier(None);
        assert_eq!(plain, render_sized(&gpu, &mut tank, format, (tw, th)));
    }
}
