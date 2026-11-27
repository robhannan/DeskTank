//! Smoke test: draw a tiny world offscreen and read pixels back. Skipped
//! (with a message) on machines without a GPU adapter, but fails when `CI`
//! is set: CI has Metal, WARP/DX12 and lavapipe. Reference-image tests are
//! in `crates/desktank/tests/render.rs`.

use std::sync::OnceLock;

use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, wgpu};
use desktank_sim::{Brush, MaterialTable, World, WorldConfig};

const W: u32 = 80;
const H: u32 = 32;

fn gpu() -> Option<&'static Gpu> {
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        open_gpu(GpuOptions {
            split_grid: Some(false),
            ..GpuOptions::default()
        })
    })
    .as_ref()
}

fn gpu_with(options: GpuOptions) -> Option<&'static Gpu> {
    static SPLIT_GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    if options.split_grid == Some(true) {
        SPLIT_GPU.get_or_init(|| open_gpu(options)).as_ref()
    } else {
        gpu()
    }
}

fn open_gpu(options: GpuOptions) -> Option<Gpu> {
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
    let img = render(gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);

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
        gpu,
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
    let img = tank.capture(gpu, (16, 8)).unwrap();
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
    let linear = render(gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
    let srgb = render(gpu, &mut tank, wgpu::TextureFormat::Rgba8UnormSrgb);
    // Backdrop and a water cell match within rounding.
    for (x, y) in [(2, 10), (9, 1)] {
        let (a, b) = (px(&linear, x, y), px(&srgb, x, y));
        for (p, q) in a.iter().zip(b) {
            assert!(p.abs_diff(q) <= 2, "({x}, {y}): {a:?} vs {b:?}");
        }
    }
}

/// OLED mode: water, air and the letterbox are exactly 0,0,0 (at 4x, flat 1x,
/// linear and sRGB targets, one pass and split), the rock still draws, and the
/// translucent click-through overlay is unchanged.
#[test]
fn oled_mode_draws_a_true_black_background() {
    let materials = MaterialTable::builtin().unwrap();
    let rock = materials.id("rock").unwrap();
    let make = || {
        let config = WorldConfig {
            width: 16,
            height: 8,
            seed: 1,
            water_level: 0.75,
        };
        let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        world.set_cell(3, 5, rock).unwrap();
        Tank::new(world, TankConfig::default())
    };
    for split in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid: Some(split),
            ..GpuOptions::default()
        }) else {
            return;
        };
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            let mut tank = make();
            let normal = render(gpu, &mut tank, format);
            assert!(
                normal.chunks(4).any(|p| p[..3] != [0, 0, 0]),
                "the normal tank has colour"
            );
            tank.set_oled(true);
            assert!(tank.oled());
            let img = render(gpu, &mut tank, format);
            // 16×8 cells in 80×32 px: scale 4, an 8 px letterbox above and
            // below. The rock covers pixels 20..24 × 20..24.
            let mut lit = 0;
            for y in 0..H {
                for x in 0..W {
                    let rock_px = (20..24).contains(&x) && (20..24).contains(&y);
                    let p = px(&img, x, y);
                    if rock_px {
                        assert!(p != [0, 0, 0], "rock ({x},{y}) is drawn as usual");
                        lit += 1;
                    } else {
                        assert_eq!(p, [0, 0, 0], "({x},{y}) split={split} {format:?}");
                    }
                }
            }
            assert_eq!(lit, 16);
            // Turning it off again restores the colours.
            tank.set_oled(false);
            assert_eq!(render(gpu, &mut tank, format), normal);
        }
    }

    let Some(gpu) = gpu() else { return };
    // Flat cells (scale 1): one pixel per cell.
    let mut tank = make();
    tank.set_oled(true);
    let img = tank.capture(gpu, (16, 8)).unwrap();
    for (i, p) in img.chunks(4).enumerate() {
        if i == 5 * 16 + 3 {
            assert!(p[..3] != [0, 0, 0], "rock cell is drawn as usual");
        } else {
            assert_eq!(p[..3], [0, 0, 0], "cell {i}");
        }
    }

    // The overlay's translucent water ignores the flag.
    let mut plain = make();
    plain.set_overlay_water_alpha(Some(0.5));
    let want = render(gpu, &mut plain, wgpu::TextureFormat::Rgba8Unorm);
    let mut oled = make();
    oled.set_overlay_water_alpha(Some(0.5));
    oled.set_oled(true);
    assert_eq!(
        render(gpu, &mut oled, wgpu::TextureFormat::Rgba8Unorm),
        want
    );
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
    let healthy = render(gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
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
    let starving = render(gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
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
        let with = render(gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
        let world = tank.world_mut();
        let depth = world.creature(id).unwrap().depth();
        let (cx, cy) = world.creature(id).unwrap().centre();
        assert!(world.remove_creature(id));
        tank.invalidate();
        let without = render(gpu, &mut tank, wgpu::TextureFormat::Rgba8Unorm);
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
        let (w, h) = (80u32, 64u32);
        let config = WorldConfig {
            width: w as usize,
            height: h as usize,
            seed: 4,
            water_level: 1.0,
        };
        let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        // Place an internal filter at the bottom so row 0 stays still water
        // and its nozzle drives a horizontal jet at row 42 (field row 5).
        assert!(
            world
                .add_device(
                    desktank_sim::DeviceKind::InternalFilter,
                    outlet_x,
                    49,
                    false
                )
                .unwrap()
                .is_some()
        );
        world.tick();
        let mut tank = Tank::new(world, TankConfig::default());
        let plain = render_sized(gpu, &mut tank, format, (w, h));
        tank.set_field_overlay(Some(desktank_render::FieldOverlay::Flow));
        let tinted = render_sized(gpu, &mut tank, format, (w, h));
        let at = |buf: &[u8], x: u32, y: u32| -> [u8; 3] {
            let i = ((y * w + x) * 4) as usize;
            [buf[i], buf[i + 1], buf[i + 2]]
        };
        let x = if outlet_x == 5 { 28 } else { 51 };
        let gain = |y| i16::from(at(&tinted, x, y)[0]) - i16::from(at(&plain, x, y)[0]);
        assert!(
            gain(43) > gain(0) && gain(43) > 0,
            "jet core is brighter than distant circulation"
        );
        // The directional arrow inside the 8x8 block (by=40..48) has its
        // arrowhead on the downstream half of the block.
        let (bx, by) = (x / 8 * 8, 40);
        let half_gain = |x0: u32, x1: u32| -> i32 {
            (by..by + 8)
                .flat_map(|y| (x0..x1).map(move |cx| (cx, y)))
                .map(|(cx, cy)| {
                    i32::from(at(&tinted, cx, cy)[0]) - i32::from(at(&plain, cx, cy)[0])
                })
                .sum()
        };
        let (left_half, right_half) = (half_gain(bx, bx + 4), half_gain(bx + 4, bx + 8));
        if outlet_x == 5 {
            assert!(
                right_half > left_half,
                "right-pointing arrow should be brighter on the right half: {left_half} vs {right_half}"
            );
        } else {
            assert!(
                left_half > right_half,
                "left-pointing arrow should be brighter on the left half: {left_half} vs {right_half}"
            );
        }
        tank.set_field_overlay(None);
        assert_eq!(plain, render_sized(gpu, &mut tank, format, (w, h)));
    }
}

/// #186: swaying leaves lean downstream in the local current. A leaf block
/// in an internal filter's jet spills over more pixels on its downstream
/// side than in still water, in either jet direction.
#[test]
fn swaying_leaves_lean_downstream_with_the_current() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (gw, gh) = (80usize, 64usize);
    let (tw, th) = (320u32, 256u32); // scale 4: quarter-cell sway is visible
    // Leaf block rows 40..46, in the jet (field row 5) driven by a filter
    // at the bottom left (rightward) or bottom right (leftward).
    let (y0, y1) = (40usize, 46usize);
    // (filter outlet x, block x0) per case; `None` is still water.
    let lean = |outlet: Option<i64>, x0: usize| -> (i32, f32) {
        let materials = MaterialTable::builtin().unwrap();
        let leaf = materials.id("leaf").unwrap();
        let config = WorldConfig {
            width: gw,
            height: gh,
            seed: 4,
            water_level: 1.0,
        };
        let mut world = World::new(config, materials).unwrap();
        if let Some(x) = outlet {
            world
                .add_device(desktank_sim::DeviceKind::InternalFilter, x, 49, false)
                .unwrap()
                .expect("internal filter fits");
        }
        let x1 = x0 + 6;
        world
            .fill_rect(
                desktank_sim::Rect { x0, y0, x1, y1 },
                leaf,
                desktank_sim::Brush::Replace,
            )
            .unwrap();
        world.tick();
        let speed = world.ecology().current.speed;
        let vx = world.current_at(x0 + 3, y0 + 3)[0] / speed;
        let mut tank = Tank::new(world, TankConfig::default());
        let px = |c: usize| u32::try_from(c * 4).unwrap();
        let (l, r) = (px(x0), px(x1));
        let rows = px(y0)..px(y1);
        // Over several sway phases, pixels changed by sway in the cell-wide
        // strip just outside the right side, minus those outside the left.
        let mut net = 0;
        for _ in 0..8 {
            for _ in 0..13 {
                tank.step();
            }
            tank.set_sway(true);
            let swayed = render_sized(gpu, &mut tank, format, (tw, th));
            tank.set_sway(false);
            tank.invalidate();
            let still = render_sized(gpu, &mut tank, format, (tw, th));
            let changed = |cols: std::ops::Range<u32>| -> i32 {
                let n = cols
                    .flat_map(|x| rows.clone().map(move |y| (x, y)))
                    .filter(|&(x, y)| {
                        let i = ((y * tw + x) * 4) as usize;
                        swayed[i..i + 3] != still[i..i + 3]
                    })
                    .count();
                i32::try_from(n).unwrap()
            };
            net += changed(r..r + 4) - changed(l - 4..l);
        }
        (net, vx)
    };
    for (outlet, x0) in [(5, 25), (74, 49)] {
        let (calm, _) = lean(None, x0);
        let (jet, vx) = lean(Some(outlet), x0);
        assert!(
            vx.abs() > 0.3,
            "outlet {outlet}: weak jet at the block ({vx})"
        );
        if vx > 0.0 {
            assert!(
                jet > calm && jet > 0,
                "rightward jet leans right: {jet} vs still {calm}"
            );
        } else {
            assert!(
                jet < calm && jet < 0,
                "leftward jet leans left: {jet} vs still {calm}"
            );
        }
    }
}

/// D27: a device is drawn as its sprite over cells drawn as water, on its
/// glass; its placement ghost is see-through, lighter than the device.
#[test]
#[allow(clippy::many_single_char_names)] // pixel maths
fn a_device_and_its_ghost_are_drawn() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let make = |filter: bool| {
        let config = WorldConfig {
            width: W as usize,
            height: H as usize,
            seed: 4,
            water_level: 1.0,
        };
        let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        if filter {
            let kind = desktank_sim::DeviceKind::InternalFilter;
            assert!(world.add_device(kind, 3, 16, false).unwrap().is_some());
        }
        Tank::new(world, TankConfig::default())
    };
    let mut plain = make(false);
    let mut with = make(true);
    let (a, b) = (
        render(gpu, &mut plain, format),
        render(gpu, &mut with, format),
    );
    // At scale 1 a pixel spans 2×2 art texels and its centre falls on a
    // texel boundary, which rasterisers round differently (lavapipe picked
    // a slot vent where Metal picked the plastic beside it). So sample a
    // cell whose whole 3×3 neighbourhood is plain body plastic in the art:
    // whichever texel a rasteriser picks there is the body colour.
    let world = with.world();
    let d = *world.devices().next().unwrap();
    let art = world.device_kinds().get(d.kind);
    let (fine, fw) = (&art.sprite.fine_frames[0], art.sprite.width * 2);
    let base = fine[(art.size.1 / 2 * 4) * fw + 3 * 4];
    let plain_cell = |cx: usize, cy: usize| {
        (cy * 4 - 4..cy * 4 + 8).all(|y| (cx * 4 - 4..cx * 4 + 8).all(|x| fine[y * fw + x] == base))
    };
    let (cx, cy) = (2..art.size.1 - 2)
        .flat_map(|cy| (2..art.size.0 - 2).map(move |cx| (cx, cy)))
        .find(|&(cx, cy)| plain_cell(cx, cy))
        .expect("a plain patch of plastic");
    let at = |d: &desktank_sim::Device| {
        let x = if d.facing_left {
            art.size.0 - 1 - cx
        } else {
            cx
        };
        (
            u32::try_from(d.x + x).unwrap(),
            u32::try_from(d.y + cy).unwrap(),
        )
    };
    let lum = |p: [u8; 3]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    // The body, grey-black plastic, over what would be blue water; open
    // water beside it is untouched.
    let (x, y) = at(&d);
    let (body, water) = (px(&b, x, y), px(&a, x, y));
    assert!(
        body[2] + 30 < water[2] && body[2] < body[0] + 20,
        "{body:?} at ({x}, {y}) over {water:?}"
    );
    assert_eq!(px(&a, 40, 16), px(&b, 40, 16));
    // A ghost on the other glass: changed, but lighter than the real body.
    let ghost = world.device_placement(desktank_sim::DeviceKind::InternalFilter, 70, 16, false);
    assert!(ghost.1 && ghost.0.facing_left);
    let (gx, gy) = at(&ghost.0);
    with.set_device_ghost(Some(ghost));
    let g = render(gpu, &mut with, format);
    assert_ne!(px(&g, gx, gy), px(&b, gx, gy));
    assert!(lum(px(&g, gx, gy)) > lum(body), "{:?}", px(&g, gx, gy));
    with.set_device_ghost(None);
    assert_eq!(render(gpu, &mut with, format), b);
}

#[test]
fn under_gravel_filter_draws_the_telescoping_tube_and_connected_floor_plates() {
    let Some(gpu) = gpu() else { return };
    let height = 52;
    let config = WorldConfig {
        width: W as usize,
        height,
        seed: 7,
        water_level: 0.9,
    };
    let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
    let gravel = world.materials().id("gravel").unwrap();
    world
        .fill_rect(
            desktank_sim::Rect {
                x0: 0,
                y0: height - 6,
                x1: W as usize,
                y1: height,
            },
            gravel,
            Brush::Replace,
        )
        .unwrap();
    let id = world
        .add_device(desktank_sim::DeviceKind::UnderGravelFilter, 40, 16, false)
        .unwrap()
        .unwrap();
    let device = *world.device(id).unwrap();
    assert_eq!(device.y, world.surface_row() + 1);
    let panel_id = world
        .add_device(desktank_sim::DeviceKind::UnderGravelPanel, 64, 16, false)
        .unwrap()
        .unwrap();
    let panel = *world.device(panel_id).unwrap();
    assert_eq!(panel.x, device.x + 28);
    assert!(world.device_status(panel_id).unwrap().connected);
    let mut tank = Tank::new(world, TankConfig::default());
    // Four pixels per cell sample the fine sprite at texel centres.
    let width = W as usize * 4;
    let frame = render_sized(
        gpu,
        &mut tank,
        wgpu::TextureFormat::Rgba8Unorm,
        (W * 4, u32::try_from(height).unwrap() * 4),
    );
    let sample_at = |x: usize, y: usize| {
        let index = (y * width + x) * 4;
        [frame[index], frame[index + 1], frame[index + 2]]
    };

    let sample = |x, y| sample_at(device.x * 4 + x, (height - 28) * 4 + y);
    let slot = sample(32, 106);
    let bevel = sample(34, 106);
    let floor_edge = sample(34, 111);
    assert!(
        floor_edge[0] <= slot[0] && slot[0] < bevel[0] && bevel[0] < bevel[2],
        "charcoal plate with dark slots at the floor: {floor_edge:?}, {slot:?}, {bevel:?}"
    );
    let [red, _, blue] = sample(34, 99);
    assert!(red > blue && red > bevel[0], "gravel just above the plate");

    let spout = sample_at(device.x * 4 + 22, device.y * 4 + 8);
    let tube = sample_at(device.x * 4 + 9, device.y * 4 + 40);
    let water = sample_at(device.x * 4 + 30, device.y * 4 + 8);
    assert!(spout[0] > water[0], "spout near the surface: {spout:?}");
    let collar = sample(6, 88);
    assert!(collar[0] > bevel[0], "frosted collar: {collar:?}");
    let stone = sample(10, 76);
    assert!(tube[0] > water[0], "clear tube highlight above the bed");
    assert!(
        stone[2] > stone[0].saturating_add(50) && stone[0] < tube[0],
        "blue air stone inside the tube: {stone:?}"
    );

    let panel_sample = |x, y| sample_at(panel.x * 4 + x, panel.y * 4 + y);
    let slot = panel_sample(7, 5);
    let bevel = panel_sample(9, 5);
    let floor_edge = panel_sample(9, 11);
    assert!(
        floor_edge[0] <= slot[0] && slot[0] < bevel[0] && bevel[0] < bevel[2],
        "panel plate beside the filter: {floor_edge:?}, {slot:?}, {bevel:?}"
    );
}

/// The magnifier zooms a single rock cell and a creature sprite inside the
/// lens rect, draws a border around it, leaves pixels outside untouched, and
/// works identically on both single-pass and split-grid (`webOS`) pipelines.
#[test]
#[allow(clippy::too_many_lines)]
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
            split_grid: Some(split_grid),
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
        // Single rock cell at (40, 22) and a creature beside it at (47, 22).
        world.set_cell(40, 22, rock).unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        world.add_creature(tetra, 47, 22).unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let plain = render_sized(gpu, &mut tank, format, (tw, th));

        let layout = tank.layout((tw, th));
        assert_eq!(layout.scale, 4);

        for zoom in [2u32, 4u32] {
            let mag = desktank_render::Magnifier {
                cell: (40, 22),
                zoom,
            };
            let ml = desktank_render::MagnifierLayout::fit((tw, th), &layout, mag).unwrap();
            tank.set_magnifier(Some(mag));
            let zoomed = render_sized(gpu, &mut tank, format, (tw, th));

            // Pixels outside the magnifying glass frame rect are byte-identical
            // to the plain frame.
            for y in 0..th {
                for x in 0..tw {
                    let in_frame = x >= ml.frame_origin.0
                        && x < ml.frame_origin.0 + ml.frame_size.0
                        && y >= ml.frame_origin.1
                        && y < ml.frame_origin.1 + ml.frame_size.1;
                    if !in_frame {
                        assert_eq!(
                            px_at(&zoomed, x, y),
                            px_at(&plain, x, y),
                            "split={split_grid} zoom={zoom}: outside frame ({x},{y}) changed"
                        );
                    }
                }
            }

            // Roundness check: the top-left corner of the frame bounding box is
            // outside the circular bezel, so the plain tank shows through there.
            assert_eq!(
                px_at(&zoomed, ml.frame_origin.0, ml.frame_origin.1),
                px_at(&plain, ml.frame_origin.0, ml.frame_origin.1),
                "split={split_grid} zoom={zoom}: top-left corner outside circular rim must stay untouched"
            );

            // Inside the circular lens, cell (40, 22) covers an exact
            // (4 * zoom)^2 block centred at `ml.centre`.
            let cell_px = layout.scale * zoom;
            let (cx, cy) = ml.centre;
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

            // The circular bezel is drawn just outside `ml.radius` (top edge).
            let rim_y = cy - ml.radius - 2 * ml.step;
            assert_ne!(
                px_at(&zoomed, cx, rim_y),
                px_at(&plain, cx, rim_y),
                "split={split_grid} zoom={zoom}: expected visible circular bezel"
            );

            // The diagonal handle is drawn extending down-right from the rim.
            let diag = ((ml.radius + 10 * ml.step) * 707) / 1000;
            let (hx, hy) = (cx + diag, cy + diag);
            assert_ne!(
                px_at(&zoomed, hx, hy),
                px_at(&plain, hx, hy),
                "split={split_grid} zoom={zoom}: expected visible magnifying-glass handle at ({hx},{hy})"
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
        assert_eq!(plain, render_sized(gpu, &mut tank, format, (tw, th)));
    }
}

/// Waterlogged driftwood renders visibly darker than dry driftwood (both free
/// and glued, at 4× scale, in single-pass and split-grid pipelines).
#[test]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn waterlogged_driftwood_renders_darker_than_dry_in_single_and_split_grid() {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (gw, gh) = (64usize, 48usize);
    let (tw, th) = (256u32, 192u32); // scale 4 (fine)

    let mut world = World::new(
        WorldConfig {
            width: gw,
            height: gh,
            seed: 7,
            water_level: 0.5,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let wood = world.materials().id("driftwood").unwrap();
    // Free driftwood in air (stays dry) vs glued driftwood underwater (waterlogs).
    // Use two glued blocks so both stay at known coordinates: one in air, one in water.
    let air_rect = desktank_sim::Rect {
        x0: 8,
        y0: 6,
        x1: 20,
        y1: 14,
    };
    let water_rect = desktank_sim::Rect {
        x0: 36,
        y0: 30,
        x1: 48,
        y1: 38,
    };
    world
        .fill_rect(air_rect, wood, desktank_sim::Brush::Replace)
        .unwrap();
    let dry_id = world.body_at(12, 10).unwrap();
    world.set_anchored(dry_id, true);
    world
        .fill_rect(water_rect, wood, desktank_sim::Brush::Replace)
        .unwrap();
    let wet_id = world.body_at(40, 34).unwrap();
    world.set_anchored(wet_id, true);

    for _ in 0..(desktank_sim::DRIFTWOOD_SOAK_EVERY * 135) {
        world.tick();
    }

    let mean_luma = |frame: &[u8], r: desktank_sim::Rect| -> f64 {
        let mut sum = 0u64;
        let mut count = 0u64;
        for cy in r.y0 + 1..r.y1 - 1 {
            for cx in r.x0 + 1..r.x1 - 1 {
                let px = (cx as u32) * 4 + 2;
                let py = (cy as u32) * 4 + 2;
                let i = ((py * tw + px) * 4) as usize;
                sum += u64::from(frame[i]) + u64::from(frame[i + 1]) + u64::from(frame[i + 2]);
                count += 3;
            }
        }
        sum as f64 / count as f64
    };

    let mut single_frame = None;
    for split_grid in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid: Some(split_grid),
            ..GpuOptions::default()
        }) else {
            return;
        };
        let mut tank = Tank::new(world.clone(), TankConfig::default());
        let frame = render_sized(gpu, &mut tank, format, (tw, th));
        let dry_luma = mean_luma(&frame, air_rect);
        let wet_luma = mean_luma(&frame, water_rect);
        assert!(
            wet_luma + 15.0 < dry_luma,
            "split={split_grid}: waterlogged wood ({wet_luma:.1}) should be visibly darker than dry ({dry_luma:.1})"
        );
        if !split_grid {
            single_frame = Some(frame);
        } else if let Some(ref single) = single_frame {
            assert_eq!(
                frame, *single,
                "split-grid driftwood must match single-pass"
            );
        }
    }
}

/// DV5 (HOB filter): above-water device footprint cells draw as air (not
/// water), the waterfall ripples the water surface near the outlet while
/// leaving the far surface calm, and fish in front depth bands render in
/// front of the back-band intake tube (both single-pass and split-grid).
#[test]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]
fn hob_filter_draws_air_above_surface_ripples_outlet_and_sorts_fish_in_front_of_tube() {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (gw, gh) = (128usize, 64usize);
    let (tw, th) = (512u32, 256u32); // scale 4 (fine)

    let make_world = |with_hob: bool| {
        let mut world = World::new(
            WorldConfig {
                width: gw,
                height: gh,
                seed: 7,
                water_level: 0.82, // surface_row = 12 (> HOB hood height 10)
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        if with_hob {
            world
                .add_device(desktank_sim::DeviceKind::HobFilter, 40, 0, false)
                .unwrap()
                .expect("HOB fits");
        }
        for _ in 0..8 {
            world.tick();
        }
        world
    };

    let px_at = |frame: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * tw + x) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
    };

    let mut single_with_fish = None;
    for split_grid in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid: Some(split_grid),
            ..GpuOptions::default()
        }) else {
            return;
        };
        let mut calm_tank = Tank::new(make_world(false), TankConfig::default());
        let calm = render_sized(gpu, &mut calm_tank, format, (tw, th));

        let mut hob_world = make_world(true);
        let dev = *hob_world.devices().next().unwrap();
        let art = hob_world
            .device_kinds()
            .get(desktank_sim::DeviceKind::HobFilter)
            .clone();
        let surface = hob_world.surface_row();
        assert!(surface > art.nozzle.unwrap().1);

        let mut hob_tank = Tank::new(hob_world.clone(), TankConfig::default());
        let with_hob = render_sized(gpu, &mut hob_tank, format, (tw, th));

        // 1. Rounded top-left corner cell of the HOB footprint (dev.x, 0) is above
        //    surface_row and its open corner pixel must match air, not dark water.
        let corner_px = px_at(&with_hob, (dev.x as u32) * 4, 0);
        let air_px = px_at(&calm, (dev.x as u32) * 4, 0);
        assert_eq!(
            corner_px, air_px,
            "split={split_grid}: transparent corner of above-water HOB cell must show air, not water"
        );

        // 2. Water surface directly below/beside the HOB spillway visibly differs
        //    from the calm surface (waterfall curtain & ripples), while the far
        //    surface stays identical to calm.
        let (sx0, sx1, _) = dev.spillway_world(&art).unwrap();
        let ripple_diffs: usize = (sx0.saturating_sub(10)..(sx1 + 10).min(gw))
            .flat_map(|cx| {
                let y0 = ((surface - 1) as u32) * 4;
                let y1 = ((surface + 1) as u32) * 4;
                (y0..y1).map(move |py| (cx as u32 * 4 + 2, py))
            })
            .filter(|&(x, y)| px_at(&with_hob, x, y) != px_at(&calm, x, y))
            .count();
        assert!(
            ripple_diffs > 20,
            "split={split_grid}: expected visible surface ripples near HOB outlet, got {ripple_diffs} differing pixels"
        );

        let far_x = ((gw - 8) as u32) * 4 + 2;
        for py in ((surface - 1) as u32) * 4..((surface + 1) as u32) * 4 {
            assert_eq!(
                px_at(&with_hob, far_x, py),
                px_at(&calm, far_x, py),
                "split={split_grid}: far surface at ({far_x}, {py}) should stay calm"
            );
        }

        // 3. A fish swimming in a front band over the back-band intake tube renders
        //    in front of the dark tube.
        let tetra = hob_world.species().id("neon_tetra").unwrap();
        let tube_col = (dev.x + 17) as u32;
        let tube_row = (dev.y + art.back_band_from.unwrap() + 10) as u32;
        let mut front_fish = None;
        for k in 0..32 {
            let id = hob_world.add_creature(tetra, 80 + k, 40).unwrap();
            if hob_world.creature(id).unwrap().band() <= 3 {
                front_fish = Some(id);
                break;
            }
        }
        let fish = front_fish.expect("spawned a front-band tetra");
        assert!(hob_world.move_creature(fish, i64::from(tube_col), i64::from(tube_row)));
        let mut fish_tank = Tank::new(hob_world, TankConfig::default());
        let with_fish = render_sized(gpu, &mut fish_tank, format, (tw, th));
        let fish_diffs: usize = ((tube_row - 2) * 4..(tube_row + 2) * 4)
            .flat_map(|py| ((tube_col - 2) * 4..(tube_col + 2) * 4).map(move |px| (px, py)))
            .filter(|&(px, py)| px_at(&with_fish, px, py) != px_at(&with_hob, px, py))
            .count();
        assert!(
            fish_diffs > 20,
            "split={split_grid}: front-band fish must draw in front of back-band HOB tube, got {fish_diffs} differing pixels"
        );
        if !split_grid {
            single_with_fish = Some(with_fish);
        } else if let Some(ref single) = single_with_fish {
            assert_eq!(
                with_fish, *single,
                "split-grid HOB + fish output must match single-pass"
            );
        }
    }
}

/// Strong 2D current at the water surface (#165) visibly disrupts/ripples the
/// surface without a HOB filter, while a deep filter whose surface current
/// stays below threshold leaves the water line pixel-identical to calm (both
/// single-pass and split-grid).
#[test]
#[allow(clippy::cast_possible_truncation)]
fn strong_surface_current_ripples_water_while_weak_current_stays_calm() {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (gw, gh) = (160usize, 128usize);
    let (tw, th) = (640u32, 512u32); // scale 4 (fine)

    let make_world = |filter_y: Option<i64>| {
        let mut world = World::new(
            WorldConfig {
                width: gw,
                height: gh,
                seed: 17,
                water_level: 0.88, // surface_row = 15
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        if let Some(y) = filter_y {
            world
                .add_device(desktank_sim::DeviceKind::InternalFilter, 0, y, false)
                .unwrap()
                .expect("internal filter fits");
        }
        for _ in 0..8 {
            world.tick();
        }
        world
    };

    let px_at = |frame: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * tw + x) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
    };

    let mut single_high = None;
    for split_grid in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid: Some(split_grid),
            ..GpuOptions::default()
        }) else {
            return;
        };
        let calm_world = make_world(None);
        let surface = calm_world.surface_row();
        let mut calm_tank = Tank::new(calm_world, TankConfig::default());
        let calm = render_sized(gpu, &mut calm_tank, format, (tw, th));

        // 1. High internal filter right beneath the surface: strong current
        //    ripples the open surface ahead of the nozzle.
        let high_world = make_world(Some(i64::try_from(surface).unwrap()));
        assert!(high_world.has_surface_current_ripples());
        let mut high_tank = Tank::new(high_world, TankConfig::default());
        let high = render_sized(gpu, &mut high_tank, format, (tw, th));

        let y0 = ((surface - 1) as u32) * 4;
        let y1 = ((surface + 1) as u32) * 4;
        let ripple_diffs: usize = (24u32..96)
            .flat_map(|cx| (y0..y1).map(move |py| (cx * 4 + 2, py)))
            .filter(|&(x, y)| px_at(&high, x, y) != px_at(&calm, x, y))
            .count();
        assert!(
            ripple_diffs > 40,
            "split={split_grid}: expected visible current-driven surface ripples, got {ripple_diffs} differing pixels"
        );

        // 2. Deep internal filter near the floor: surface current stays below
        //    threshold, so the entire water surface stays identical to calm.
        let deep_world = make_world(Some(96));
        assert!(!deep_world.has_surface_current_ripples());
        let mut deep_tank = Tank::new(deep_world, TankConfig::default());
        let deep = render_sized(gpu, &mut deep_tank, format, (tw, th));
        for cx in 24..gw {
            let px = (cx as u32) * 4 + 2;
            for py in y0..y1 {
                assert_eq!(
                    px_at(&deep, px, py),
                    px_at(&calm, px, py),
                    "split={split_grid}: deep filter surface at ({px}, {py}) should stay calm"
                );
            }
        }

        if !split_grid {
            single_high = Some(high);
        } else if let Some(ref single) = single_high {
            assert_eq!(
                high, *single,
                "split-grid surface-current output must match single-pass"
            );
        }
    }
}

#[test]
fn rigid_bodies_occlude_sprites_behind_them_in_both_pipelines() {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let size = (320, 192);
    for split_grid in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid: Some(split_grid),
            ..GpuOptions::default()
        }) else {
            return;
        };
        let materials = MaterialTable::builtin().unwrap();
        let rock = materials.id("rock").unwrap();
        let mut world = World::new(
            WorldConfig {
                width: 80,
                height: 48,
                seed: 1,
                water_level: 1.0,
            },
            materials,
        )
        .unwrap();
        world.paint(40, 24, 8, rock, Brush::Replace).unwrap();
        world.tick();
        let (front, back) = world.rigid_bands(24 * 80 + 40).unwrap();
        assert!(front > 0 && back < desktank_sim::DEPTH_BANDS - 1);
        let tetra = world.species().id("neon_tetra").unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let bare = render_sized(gpu, &mut tank, format, size);

        for behind in [false, true] {
            let world = tank.world_mut();
            let fish = (0..64)
                .find_map(|_| {
                    let id = world.add_creature(tetra, 15, 24).unwrap();
                    let band = world.creature(id).unwrap().band();
                    if (behind && band > back) || (!behind && band < front) {
                        Some(id)
                    } else {
                        world.remove_creature(id);
                        None
                    }
                })
                .expect("seeded tetras include a fish on each side of the rock");
            assert!(world.move_creature(fish, 40, 24));

            let with_fish = render_sized(gpu, &mut tank, format, size);
            let changed = (22 * 4..27 * 4).any(|y| {
                let start = (y * size.0 as usize + 36 * 4) * 4;
                let end = start + 9 * 4 * 4;
                with_fish[start..end] != bare[start..end]
            });
            assert_eq!(changed, !behind, "split={split_grid}, behind={behind}");
            if behind {
                let world = tank.world_mut();
                let water = world.materials().id("water").unwrap();
                world.paint(40, 24, 12, water, Brush::Replace).unwrap();
                tank.step();
                let uncovered = render_sized(gpu, &mut tank, format, size);
                tank.world_mut().remove_creature(fish);
                let empty = render_sized(gpu, &mut tank, format, size);
                assert!(uncovered != empty, "erasing the rock reveals the fish");
            } else {
                tank.world_mut().remove_creature(fish);
            }
        }
    }
}

/// A tank of `width`×`height` cells with plants, a rigid rock, a creature
/// and the nutrient field overlay, all varying with `seed`, ticked so the
/// plants and rigid body exist.
fn busy_tank(width: usize, height: usize, seed: u64) -> Tank {
    let materials = MaterialTable::builtin().unwrap();
    let leaf = materials.id("leaf").unwrap();
    let rock = materials.id("rock").unwrap();
    let mut world = World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level: 1.0,
        },
        materials,
    )
    .unwrap();
    let (x, y) = (width / 4 + usize::try_from(seed).unwrap(), height * 3 / 4);
    let (w, h) = (
        i64::try_from(width).unwrap(),
        i64::try_from(height).unwrap(),
    );
    world
        .fill_rect(
            desktank_sim::Rect {
                x0: x,
                y0: y - 3,
                x1: x + 4,
                y1: y,
            },
            leaf,
            Brush::Replace,
        )
        .unwrap();
    world
        .paint(w * 3 / 4, h / 2, 4, rock, Brush::Replace)
        .unwrap();
    let tetra = world.species().id("neon_tetra").unwrap();
    world.add_creature(tetra, w / 2, h / 3).unwrap();
    for _ in 0..4 {
        world.tick();
    }
    let mut tank = Tank::new(world, TankConfig::default());
    tank.set_field_overlay(Some(desktank_render::FieldOverlay::Scalar(
        desktank_sim::FieldKind::Nutrient,
    )));
    tank
}

/// A replacement tank that takes over the old tank's GPU renderer draws
/// exactly what a fresh tank would, at the same grid size and across
/// grid-size changes, without rebuilding the renderer.
#[test]
fn a_tank_reusing_a_transferred_renderer_matches_a_fresh_tank() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    // Same size, then a larger grid, a smaller one, and back to the first.
    let grids = [
        (80, 32, 1),
        (80, 32, 2),
        (96, 48, 3),
        (64, 24, 4),
        (80, 32, 5),
    ];
    let mut previous = busy_tank(grids[0].0, grids[0].1, grids[0].2);
    render(gpu, &mut previous, format);
    for (width, height, seed) in grids {
        let mut fresh = busy_tank(width, height, seed);
        let expected = render(gpu, &mut fresh, format);
        let mut reused = busy_tank(width, height, seed);
        reused.take_gpu(&mut previous);
        assert!(reused.frame_wanted());
        let actual = render(gpu, &mut reused, format);
        assert!(
            actual == expected,
            "grid {width}×{height} seed {seed}: reused renderer drew different pixels"
        );
        // A second frame stays identical (nothing stale left to re-upload).
        assert!(render(gpu, &mut reused, format) == expected);
        previous = reused;
    }
}

#[test]
fn overlay_aligns_to_the_floor_and_composites_water_and_sprites() {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let size = (W, 40);
    let rgba = |img: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * W + x) * 4) as usize;
        img[i..i + 4].try_into().unwrap()
    };
    for split_grid in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid: Some(split_grid),
            ..GpuOptions::default()
        }) else {
            return;
        };
        let materials = MaterialTable::builtin().unwrap();
        let sand = materials.id("sand").unwrap();
        let mut world = World::new(
            WorldConfig {
                width: 16,
                height: 8,
                seed: 1,
                water_level: 0.75,
            },
            materials,
        )
        .unwrap();
        world.set_cell(3, 7, sand).unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        assert_eq!(tank.overlay_water_alpha(), None);
        assert_eq!(tank.layout(size).offset, (8, 4));
        let opaque = render_sized(gpu, &mut tank, format, size);
        assert!(opaque.as_chunks::<4>().0.iter().all(|p| p[3] == 255));

        for (alpha, expected) in [(0.0, 0), (0.25, 64)] {
            tank.set_overlay_water_alpha(Some(alpha));
            assert_eq!(tank.layout(size).offset, (8, 8));
            let overlay = render_sized(gpu, &mut tank, format, size);
            for (x, y) in [(0, 24), (12, 7), (12, 9)] {
                assert_eq!(rgba(&overlay, x, y), [0; 4], "clear at ({x}, {y})");
            }
            // Open water and the water visible at the top corner of the sand cell.
            for (x, y) in [(12, 26), (20, 36)] {
                let got = rgba(&overlay, x, y);
                assert_eq!(got[3], expected, "water at ({x}, {y})");
                let before = rgba(&opaque, x, y - 4);
                for (value, base) in got[..3].iter().zip(before) {
                    assert!((f32::from(*value) - f32::from(base) * alpha).abs() <= 1.0);
                }
                if alpha == 0.0 {
                    assert_eq!(got, [0; 4]);
                }
            }
            assert_eq!(rgba(&overlay, 21, 39), rgba(&opaque, 21, 35));

            let fish = tank.world_mut().add_creature(tetra, 11, 5).unwrap();
            let with_fish = render_sized(gpu, &mut tank, format, size);
            let sprite_texels = overlay
                .as_chunks::<4>()
                .0
                .iter()
                .zip(with_fish.as_chunks::<4>().0)
                .filter(|(before, after)| before[3] == expected && after[3] == 255)
                .count();
            assert!(sprite_texels > 0, "split={split_grid}, alpha={alpha}");
            tank.world_mut().remove_creature(fish);
        }
        tank.set_overlay_water_alpha(None);
        assert_eq!(opaque, render_sized(gpu, &mut tank, format, size));
    }
}

/// Clown vomit gravel renders each pebble in its variant's saturated neon
/// colour (more than the two low-bit variants), identically on single-pass
/// and split-grid pipelines.
#[test]
fn clown_vomit_gravel_renders_multiple_saturated_pebble_colours() {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (tw, th) = (320u32, 180u32);
    let mut single: Option<Vec<u8>> = None;
    for split_grid in [false, true] {
        let Some(gpu) = gpu_with(GpuOptions {
            split_grid: Some(split_grid),
            ..GpuOptions::default()
        }) else {
            return;
        };
        let materials = MaterialTable::builtin().unwrap();
        let gravel = materials.id("clown_vomit_gravel").unwrap();
        let mut world = World::new(
            WorldConfig {
                width: 80,
                height: 45,
                seed: 1,
                water_level: 1.0,
            },
            materials,
        )
        .unwrap();
        world
            .fill_rect(
                desktank_sim::Rect {
                    x0: 10,
                    y0: 36,
                    x1: 70,
                    y1: 45,
                },
                gravel,
                Brush::Replace,
            )
            .unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let img = render_sized(gpu, &mut tank, format, (tw, th));
        if let Some(prev) = &single {
            assert_eq!(&img, prev, "split-grid must match single-grid");
        } else {
            single = Some(img.clone());
        }
        let (mut has_green, mut has_yellow, mut has_pink_or_violet, mut has_blue) =
            (false, false, false, false);
        for y in (36 * 4)..(45 * 4) {
            for x in (10 * 4)..(70 * 4) {
                let i = ((y * tw + x) * 4) as usize;
                let (r, g, b) = (img[i], img[i + 1], img[i + 2]);
                if g > 180 && r < 140 && b < 100 {
                    has_green = true;
                }
                if r > 180 && g > 160 && b < 80 {
                    has_yellow = true;
                }
                if r > 150 && b > 140 && g < 100 {
                    has_pink_or_violet = true;
                }
                if b > 180 && r < 80 {
                    has_blue = true;
                }
            }
        }
        assert!(
            has_green && has_yellow && has_pink_or_violet && has_blue,
            "missing saturated pebble colours: green={has_green} yellow={has_yellow} pink_or_violet={has_pink_or_violet} blue={has_blue}"
        );
    }
}

#[test]
fn the_species_card_is_drawn_on_top_of_the_tank_and_the_lens() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (tw, th) = (320u32, 180u32);
    let world = |with_creature: bool| {
        let materials = MaterialTable::builtin().unwrap();
        let rock = materials.id("rock").unwrap();
        let config = WorldConfig {
            width: 80,
            height: 45,
            seed: 7,
            water_level: 1.0,
        };
        let mut world = World::new(config, materials).unwrap();
        world
            .fill_rect(
                desktank_sim::Rect {
                    x0: 0,
                    y0: 30,
                    x1: 80,
                    y1: 45,
                },
                rock,
                Brush::Replace,
            )
            .unwrap();
        if with_creature {
            // Under the card's top-left corner.
            let tetra = world.species().id("neon_tetra").unwrap();
            world.add_creature(tetra, 14, 8).unwrap();
        }
        world
    };
    let mut tank = Tank::new(world(true), TankConfig::default());
    let guppy = tank.world().species().id("guppy").unwrap();
    let plain = render_sized(gpu, &mut tank, format, (tw, th));

    tank.set_card(Some(desktank_render::Card::Creature(guppy, None)));
    let layout = tank.card_layout((tw, th)).unwrap();
    // No palette: top-left, shrunk to fit the 320x180 target.
    assert_eq!((layout.scale, layout.origin), (1, (2, 2)));
    let (cw, ch) = layout.size();
    assert!(layout.origin.1 + ch <= th && ch > 100, "{layout:?}");
    let carded = render_sized(gpu, &mut tank, format, (tw, th));
    let at = |img: &[u8], x: u32, y: u32| px_at(img, tw, x, y);
    let (x0, y0) = layout.origin;
    assert_eq!(at(&carded, x0, y0), [0x2a, 0x36, 0x48], "frame");
    assert_eq!(
        at(&carded, x0 + 2, y0 + 2),
        [0x0b, 0x12, 0x1c],
        "background"
    );
    assert_eq!(at(&carded, x0 + cw - 1, y0 + ch - 1), [0x2a, 0x36, 0x48]);

    // Outside the card the tank is untouched.
    let mut changed_outside = 0;
    for y in 0..th {
        for x in 0..tw {
            let inside = x >= x0 && x < x0 + cw && y >= y0 && y < y0 + ch;
            if !inside && at(&plain, x, y) != at(&carded, x, y) {
                changed_outside += 1;
            }
        }
    }
    assert_eq!(changed_outside, 0);

    // Nothing under the card shows through: the same card over a tank with
    // no creature there draws the same pixels.
    let mut bare = Tank::new(world(false), TankConfig::default());
    bare.set_card(Some(desktank_render::Card::Creature(guppy, None)));
    let bare = render_sized(gpu, &mut bare, format, (tw, th));
    for y in y0..y0 + ch {
        for x in x0..x0 + cw {
            assert_eq!(at(&bare, x, y), at(&carded, x, y), "({x}, {y})");
        }
    }

    // A magnifier lens over the card doesn't draw into it.
    tank.set_magnifier(Some(desktank_render::Magnifier {
        cell: (30, 20),
        zoom: 2,
    }));
    let lens = render_sized(gpu, &mut tank, format, (tw, th));
    let ml = desktank_render::MagnifierLayout::fit(
        (tw, th),
        &tank.layout((tw, th)),
        tank.magnifier().unwrap(),
    )
    .unwrap();
    assert!(
        ml.lens_origin.0 < x0 + cw && ml.lens_origin.1 < y0 + ch,
        "lens meets the card"
    );
    assert_ne!(lens, carded, "the lens drew");
    for y in y0..y0 + ch {
        for x in x0..x0 + cw {
            assert_eq!(at(&carded, x, y), at(&lens, x, y), "({x}, {y})");
        }
    }

    // Hiding the card brings the tank back.
    tank.set_magnifier(None);
    tank.set_card(None);
    assert_eq!(tank.card_layout((tw, th)), None);
    assert_eq!(render_sized(gpu, &mut tank, format, (tw, th)), plain);
}

#[test]
fn the_inspect_card_is_drawn_beside_its_creature_without_input() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (tw, th) = (640u32, 360u32);
    let materials = MaterialTable::builtin().unwrap();
    let config = WorldConfig {
        width: 80,
        height: 45,
        seed: 7,
        water_level: 1.0,
    };
    let mut world = World::new(config, materials).unwrap();
    let tetra = world.species().id("neon_tetra").unwrap();
    let id = world.add_creature(tetra, 14, 20).unwrap();
    let mut tank = Tank::new(world, TankConfig::default());
    // A screensaver: no palette, no input, just the tank.
    tank.set_saver_mode(true);
    let plain = render_sized(gpu, &mut tank, format, (tw, th));

    for page in [
        desktank_render::InspectPage::Live,
        desktank_render::InspectPage::Species,
    ] {
        tank.set_inspected(Some(desktank_render::Inspected::Creature(id)));
        tank.set_inspect_page(page);
        let layout = tank.inspect_layout((tw, th)).unwrap();
        let (cw, ch) = layout.size();
        let (x0, y0) = layout.origin;
        // Right of the tetra (at cell 14 of 80, scale 8), inside the target.
        let centre = tank.world().creature(id).unwrap().centre();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tetra_x = (centre.0 * 8.0) as u32;
        assert!(x0 > tetra_x, "{page:?}: {layout:?}");
        assert!(x0 + cw <= tw && y0 + ch <= th, "{page:?}: {layout:?}");
        let carded = render_sized(gpu, &mut tank, format, (tw, th));
        let at = |img: &[u8], x: u32, y: u32| px_at(img, tw, x, y);
        assert_eq!(at(&carded, x0, y0), [0x2a, 0x36, 0x48], "{page:?} frame");
        assert_eq!(
            at(&carded, x0 + cw - 1, y0 + ch - 1),
            [0x2a, 0x36, 0x48],
            "{page:?} frame"
        );
        assert_eq!(
            at(&carded, x0 + layout.scale, y0 + ch - 2 * layout.scale),
            [0x0b, 0x12, 0x1c],
            "{page:?} background"
        );
        for y in 0..th {
            for x in 0..tw {
                let inside = x >= x0 && x < x0 + cw && y >= y0 && y < y0 + ch;
                if !inside {
                    assert_eq!(at(&plain, x, y), at(&carded, x, y), "({x}, {y})");
                }
            }
        }
    }

    // Removing it brings the tank back.
    tank.set_inspected(None);
    assert_eq!(tank.inspect_layout((tw, th)), None);
    assert_eq!(render_sized(gpu, &mut tank, format, (tw, th)), plain);
}

/// Pixel `(x, y)` of a `width`-pixel-wide RGBA image.
fn px_at(img: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * width + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

/// A creature's inspect card shows its need meters (a short red feeding bar
/// just after a first bite, a green social bar, a green chevron), in two
/// columns on a desktop-sized target and one on a phone-sized one. Writes
/// both frames to `target/shots/inspect-need-bars/` when `INSPECT_SHOTS` is
/// set.
#[test]
fn the_inspect_card_shows_need_meters_in_two_columns_or_one() {
    #[path = "../src/testing.rs"]
    mod testing;

    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let materials = MaterialTable::builtin().unwrap();
    let food = materials.id("flake_food").unwrap();
    let mut world = World::new(
        WorldConfig {
            width: 80,
            height: 45,
            seed: 7,
            water_level: 1.0,
        },
        materials,
    )
    .unwrap();
    let tetra = world.species().id("neon_tetra").unwrap();
    let ids = [(30, 20), (34, 20), (38, 20)].map(|(x, y)| world.add_creature(tetra, x, y).unwrap());
    let mut tank = Tank::new(world, TankConfig::default());
    tank.set_saver_mode(true);
    tank.set_inspected(Some(desktank_render::Inspected::Creature(ids[0])));
    // Settle, then feed the inspected fish and let it take its first bite.
    for _ in 0..250 {
        tank.step();
    }
    let (x, y) = tank.world().creature(ids[0]).unwrap().centre();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    tank.world_mut()
        .set_cell(x as usize, y as usize, food)
        .unwrap();
    for _ in 0..3 {
        tank.step();
    }
    let (red, green, track) = ([0xd8, 0x40, 0x3c], [0x4c, 0xc0, 0x6e], [0x16, 0x22, 0x34]);
    let mut heights = Vec::new();
    for (name, target) in [("desktop", (1280u32, 720u32)), ("narrow", (360, 640))] {
        let img = render_sized(gpu, &mut tank, format, target);
        let at = tank.inspect_layout(target).unwrap();
        let (cw, ch) = at.size();
        heights.push(ch / at.scale);
        let colour_in_card = |c: [u8; 3]| {
            (at.origin.1..at.origin.1 + ch)
                .flat_map(|py| (at.origin.0..at.origin.0 + cw).map(move |px_| (px_, py)))
                .filter(|&(px_, py)| px_at(&img, target.0, px_, py) == c)
                .count()
        };
        // Pixels per card pixel is a whole scale, so the meters are crisp.
        let s = usize::try_from(at.scale).unwrap();
        assert!(
            colour_in_card(red) >= 6 * 5 * s * s,
            "{name}: short red fill"
        );
        assert!(
            colour_in_card(green) >= 7 * s * s,
            "{name}: the rising chevron"
        );
        assert!(colour_in_card(track) > 0, "{name}: track");
        if std::env::var_os("INSPECT_SHOTS").is_some() {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/shots/inspect-need-bars");
            std::fs::create_dir_all(&dir).unwrap();
            // Alpha is opaque in the target already.
            std::fs::write(
                dir.join(format!("tank-{name}.png")),
                testing::png(&img, target),
            )
            .unwrap();
        }
    }
    assert!(heights[1] > heights[0], "one column is taller: {heights:?}");
}

// The camera (D29): the tank drawn offscreen at its integer layout, then a
// view of it resampled over the target.

/// An 80×45 tank with a 6×6 rock block at cells (30..36, 20..26) and a
/// tetra at (45, 20); drawn into a 640×360 target (scale 8).
fn camera_tank() -> (Tank, desktank_sim::CreatureId) {
    let materials = MaterialTable::builtin().unwrap();
    let rock = materials.id("rock").unwrap();
    let mut world = World::new(
        WorldConfig {
            width: 80,
            height: 45,
            seed: 7,
            water_level: 1.0,
        },
        materials,
    )
    .unwrap();
    world
        .fill_rect(
            desktank_sim::Rect {
                x0: 30,
                y0: 20,
                x1: 36,
                y1: 26,
            },
            rock,
            Brush::Replace,
        )
        .unwrap();
    let tetra = world.species().id("neon_tetra").unwrap();
    let id = world.add_creature(tetra, 45, 20).unwrap();
    let mut tank = Tank::new(world, TankConfig::default());
    tank.set_saver_mode(true);
    (tank, id)
}

const CAMERA_TARGET: (u32, u32) = (640, 360);

fn colour_distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(&a, b)| u32::from(a.abs_diff(b)))
        .sum()
}

#[test]
fn clearing_the_camera_draws_exactly_the_plain_tank() {
    let Some(gpu) = gpu() else { return };
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let (mut plain, _) = camera_tank();
        let expected = render_sized(gpu, &mut plain, format, CAMERA_TARGET);

        let (mut tank, _) = camera_tank();
        // A frame before the camera, with it, and after it goes.
        assert!(render_sized(gpu, &mut tank, format, CAMERA_TARGET) == expected);
        tank.set_camera(Some(tank.camera_cover(CAMERA_TARGET, (40.0, 22.0), 2.0)));
        assert!(tank.frame_wanted(), "setting a camera asks for a frame");
        let zoomed = render_sized(gpu, &mut tank, format, CAMERA_TARGET);
        assert!(zoomed != expected, "{format:?}: the zoomed view differs");
        tank.set_camera(None);
        assert!(tank.frame_wanted(), "clearing it asks for a frame");
        assert!(
            render_sized(gpu, &mut tank, format, CAMERA_TARGET) == expected,
            "{format:?}: no camera draws the plain picture, byte for byte"
        );
    }
}

#[test]
fn the_full_view_camera_reproduces_the_plain_picture() {
    let Some(gpu) = gpu() else { return };
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
    ] {
        // A letterboxed target: 80×45 cells at scale 8 in 700×400.
        let target = (700, 400);
        let (mut plain, _) = camera_tank();
        let expected = render_sized(gpu, &mut plain, format, target);
        let (mut tank, _) = camera_tank();
        tank.set_camera(Some(tank.camera_fit(target)));
        let fit = render_sized(gpu, &mut tank, format, target);
        // The sharp filter picks each pixel exactly at one to one, so the
        // letterbox and the tank come back unchanged (allow a level for GPUs
        // that blend the exact centre).
        let worst = fit
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 1, "{format:?}: fit camera differs by {worst}");
    }
}

#[test]
fn a_zoomed_camera_puts_a_cell_where_the_mapping_says() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = CAMERA_TARGET;
    let (mut plain, _) = camera_tank();
    let plain_img = render_sized(gpu, &mut plain, format, target);
    // The plain picture's rock (cell 32, 22 at scale 8) and open water.
    let rock = px_at(&plain_img, target.0, 32 * 8 + 4, 22 * 8 + 4);
    let water = px_at(&plain_img, target.0, 60 * 8, 10 * 8);
    assert!(colour_distance(rock, water) > 60, "{rock:?} {water:?}");

    let (mut tank, _) = camera_tank();
    // Zoom 2.5 on the rock block's centre: 20 pixels per cell.
    let view = tank.camera_cover(target, (33.0, 23.0), 2.5);
    tank.set_camera(Some(view));
    assert_eq!(tank.camera(), Some(view));
    let img = render_sized(gpu, &mut tank, format, target);
    let at = |x: f64, y: f64| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        px_at(&img, target.0, x as u32, y as u32)
    };
    // The block's corners and centre on screen.
    let (left, top) = tank.grid_to_screen(target, (30.0, 20.0));
    let (right, bottom) = tank.grid_to_screen(target, (36.0, 26.0));
    assert!((right - left - 120.0).abs() < 0.01 && (bottom - top - 120.0).abs() < 0.01);
    let (cx, cy) = tank.grid_to_screen(target, (33.0, 23.0));
    assert!((cx - 320.0).abs() < 0.01 && (cy - 180.0).abs() < 0.01);
    // Pixels inside the block read as rock, pixels past its edge as water.
    let nearest = |p: [u8; 3]| colour_distance(p, rock) < colour_distance(p, water);
    for (x, y) in [
        (cx, cy),
        (left + 4.0, top + 4.0),
        (right - 4.0, bottom - 4.0),
        (left + 4.0, bottom - 4.0),
    ] {
        assert!(
            nearest(at(x, y)),
            "({x}, {y}) should be rock: {:?}",
            at(x, y)
        );
    }
    for (x, y) in [
        (left - 4.0, cy),
        (right + 4.0, cy),
        (cx, top - 4.0),
        (cx, bottom + 4.0),
    ] {
        assert!(
            !nearest(at(x, y)),
            "({x}, {y}) should be water: {:?}",
            at(x, y)
        );
    }
    // And screen_to_grid undoes it, so a click lands on the right cell.
    let (gx, gy) = tank.screen_to_grid(target, (left + 5.0, top + 5.0));
    assert_eq!((gx.floor(), gy.floor()), (30.0, 20.0));
}

/// The sharp-bilinear filter on the CPU: each source pixel is a flat block
/// with about one output pixel of blend at its edges. `view` is in cells of
/// `scale` source pixels (the grid at the source's origin); `src` and the
/// result are `size` pixels of RGBA.
fn sharp_bilinear(
    src: &[u8],
    size: (u32, u32),
    scale: f64,
    view: &desktank_render::ViewRect,
) -> Vec<u8> {
    let (width, height) = (f64::from(size.0), f64::from(size.1));
    let step = (
        f64::from(view.w) * scale / width,
        f64::from(view.h) * scale / height,
    );
    let inverse = (1.0 / step.0, 1.0 / step.1);
    let tap = |x: i64, y: i64, c: usize| {
        let x = usize::try_from(x.clamp(0, i64::from(size.0) - 1)).unwrap();
        let y = usize::try_from(y.clamp(0, i64::from(size.1) - 1)).unwrap();
        f64::from(src[(y * size.0 as usize + x) * 4 + c])
    };
    let axis = |pos: f64, k: f64| {
        let flat_half = (0.5 - 0.5 / k).max(0.0);
        let from_centre = pos - pos.floor() - 0.5;
        let blend = (from_centre - from_centre.clamp(-flat_half, flat_half)) * k + 0.5;
        let t = pos.floor() + blend - 0.5;
        (t.floor(), t - t.floor())
    };
    let mut out = Vec::with_capacity(src.len());
    for y in 0..size.1 {
        for x in 0..size.0 {
            let sx = f64::from(view.x) * scale + (f64::from(x) + 0.5) * step.0;
            let sy = f64::from(view.y) * scale + (f64::from(y) + 0.5) * step.1;
            let ((cx, wx), (cy, wy)) = (axis(sx, inverse.0), axis(sy, inverse.1));
            #[allow(clippy::cast_possible_truncation)]
            let (cx, cy) = (cx as i64, cy as i64);
            for c in 0..4 {
                let top = tap(cx, cy, c) * (1.0 - wx) + tap(cx + 1, cy, c) * wx;
                let bottom = tap(cx, cy + 1, c) * (1.0 - wx) + tap(cx + 1, cy + 1, c) * wx;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                out.push((top * (1.0 - wy) + bottom * wy).round() as u8);
            }
        }
    }
    out
}

#[test]
fn the_zoom_filter_is_sharp_bilinear() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = CAMERA_TARGET;
    // The picture the camera resamples: the plain tank at scale 8.
    let (mut plain, _) = camera_tank();
    let source = render_sized(gpu, &mut plain, format, target);
    // A fractional zoom and pan (about 20.3 pixels per cell).
    let (mut tank, _) = camera_tank();
    let view = tank.camera_cover(target, (33.3, 23.1), 2.537);
    tank.set_camera(Some(view));
    let img = render_sized(gpu, &mut tank, format, target);
    let expected = sharp_bilinear(&source, target, 8.0, &view);
    let off = img
        .iter()
        .zip(&expected)
        .filter(|(a, b)| a.abs_diff(**b) > 2)
        .count();
    assert!(
        off * 1000 < img.len(),
        "{off} of {} channel values are more than 2 from the reference",
        img.len()
    );
    // It isn't plain bilinear: that smears every source pixel edge over
    // two output pixels, so far fewer pixels would match the flat blocks.
    let mut flat = 0;
    for y in 0..target.1 {
        for x in 1..target.0 {
            if px_at(&img, target.0, x, y) == px_at(&img, target.0, x - 1, y) {
                flat += 1;
            }
        }
    }
    assert!(
        flat * 10 > target.0 * target.1 * 7,
        "{flat} flat pixel pairs"
    );
}

#[test]
fn the_inspect_card_stays_beside_its_creature_under_a_zoomed_camera() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = CAMERA_TARGET;
    let (mut tank, id) = camera_tank();
    tank.set_inspected(Some(desktank_render::Inspected::Creature(id)));
    let plain_card = tank.inspect_layout(target).unwrap();

    let centre = tank.world().creature(id).unwrap().centre();
    let view = tank.camera_cover(target, centre, 2.5);
    tank.set_camera(Some(view));
    let card = tank.inspect_layout(target).unwrap();
    // The same crisp scale as without a camera, but moved with the creature.
    assert_eq!(card.scale, plain_card.scale);
    assert_eq!(card.image, plain_card.image);
    assert!(card.origin != plain_card.origin, "{card:?} {plain_card:?}");
    let (cx, cy) = tank.grid_to_screen(target, (f64::from(centre.0), f64::from(centre.1)));
    assert!(
        (cx - 320.0).abs() < 1.0 && (cy - 180.0).abs() < 1.0,
        "{cx} {cy}"
    );
    // To its right, past the creature's mapped edge, vertically centred on
    // it, and inside the target.
    let (size, (x0, y0)) = (card.size(), card.origin);
    assert!(f64::from(x0) > cx, "{card:?} vs {cx}");
    assert!(x0 + size.0 <= target.0 && y0 + size.1 <= target.1);
    let card_mid = f64::from(y0) + f64::from(size.1) / 2.0;
    assert!((card_mid - cy).abs() < 2.0, "{card_mid} vs {cy}");

    // It draws there, in screen space on top of the zoomed tank.
    let img = render_sized(gpu, &mut tank, format, target);
    assert_eq!(px_at(&img, target.0, x0, y0), [0x2a, 0x36, 0x48], "frame");
    assert_eq!(
        px_at(&img, target.0, x0 + size.0 - 1, y0 + size.1 - 1),
        [0x2a, 0x36, 0x48],
        "frame"
    );
    // Hit tests follow it too.
    assert!(card.contains(f64::from(x0) + 1.0, f64::from(y0) + 1.0));
}

#[test]
fn the_overlay_draws_over_the_camera_in_screen_space() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = CAMERA_TARGET;
    let (mut tank, _) = camera_tank();
    tank.set_camera(Some(tank.camera_cover(target, (33.0, 23.0), 2.5)));
    let bare = render_sized(gpu, &mut tank, format, target);
    // A brush outline over the rock block's cells, 20 pixels each.
    tank.set_cursor(Some(desktank_render::BrushPreview {
        cell: (33, 23),
        radius: 3,
    }));
    let brushed = render_sized(gpu, &mut tank, format, target);
    // The disc's leftmost cell is 3 cells left of the centre: it is
    // brightened where it sits on screen, and the far corner is untouched.
    let (left, cy) = tank.grid_to_screen(target, (30.0, 23.5));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x, y) = ((left + 10.0) as u32, cy as u32);
    let (a, b) = (
        px_at(&bare, target.0, x, y),
        px_at(&brushed, target.0, x, y),
    );
    assert!(b[0] > a[0] && b[1] > a[1] && b[2] > a[2], "{a:?} -> {b:?}");
    assert_eq!(
        px_at(&bare, target.0, 5, 5),
        px_at(&brushed, target.0, 5, 5)
    );
    // The magnifier's lens draws over the camera's picture too.
    tank.set_cursor(None);
    tank.set_magnifier(Some(desktank_render::Magnifier {
        cell: (33, 23),
        zoom: 2,
    }));
    let lensed = render_sized(gpu, &mut tank, format, target);
    let changed = lensed.iter().zip(&bare).filter(|(a, b)| a != b).count();
    assert!(changed > 20_000, "{changed} bytes changed under the lens");
    assert_eq!(px_at(&bare, target.0, 5, 5), px_at(&lensed, target.0, 5, 5));
    // Clearing the camera restores the plain picture.
    tank.set_magnifier(None);
    tank.set_camera(None);
    let (mut plain, _) = camera_tank();
    assert!(
        render_sized(gpu, &mut tank, format, target)
            == render_sized(gpu, &mut plain, format, target)
    );
}

/// Writes the camera screenshots to `target/shots/camera/` when
/// `DESKTANK_SHOTS` is set: the full view, and 2.5× on a creature.
#[test]
fn camera_screenshots() {
    let Some(gpu) = gpu() else { return };
    if std::env::var_os("DESKTANK_SHOTS").is_none() {
        return;
    }
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = (1280, 720);
    let materials = MaterialTable::builtin().unwrap();
    let rock = materials.id("rock").unwrap();
    let leaf = materials.id("leaf").unwrap();
    let mut world = World::new(
        WorldConfig {
            width: 160,
            height: 90,
            seed: 11,
            water_level: 1.0,
        },
        materials,
    )
    .unwrap();
    world.paint(110, 80, 9, rock, Brush::Replace).unwrap();
    for (x, h) in [(20, 14), (26, 9), (130, 12)] {
        world
            .fill_rect(
                desktank_sim::Rect {
                    x0: x,
                    y0: 88 - h,
                    x1: x + 3,
                    y1: 88,
                },
                leaf,
                Brush::Replace,
            )
            .unwrap();
    }
    let names: Vec<String> = world.species().iter().map(|s| s.name.clone()).collect();
    let mut focus = None;
    for (i, name) in (0..6_i64).zip(&names) {
        let sp = world.species().id(name).unwrap();
        let id = world
            .add_creature(sp, 25 + 22 * i, 25 + 9 * (i % 3))
            .unwrap();
        focus = focus.or(Some(id));
    }
    for _ in 0..20 {
        world.tick();
    }
    let mut tank = Tank::new(world, TankConfig::default());
    tank.set_saver_mode(true);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots/camera");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, img: &[u8]| {
        let file = std::fs::File::create(dir.join(name)).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), target.0, target.1);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(img).unwrap();
    };
    tank.set_camera(Some(tank.camera_fit(target)));
    write("full.png", &render_sized(gpu, &mut tank, format, target));

    let id = focus.unwrap();
    let centre = tank.world().creature(id).unwrap().centre();
    tank.set_camera(Some(tank.camera_cover(target, centre, 2.5)));
    tank.set_inspected(Some(desktank_render::Inspected::Creature(id)));
    write(
        "zoom_2_5.png",
        &render_sized(gpu, &mut tank, format, target),
    );
}

// The screensaver camera tour (D29), driven by the tank as a screensaver host
// would: a target size, `set_tour`, and then only simulation steps and draws.

/// A 160×90 tank with a rock, a few leaves and eight creatures, for the tour.
fn tour_tank() -> Tank {
    let materials = MaterialTable::builtin().unwrap();
    let rock = materials.id("rock").unwrap();
    let leaf = materials.id("leaf").unwrap();
    let mut world = World::new(
        WorldConfig {
            width: 160,
            height: 90,
            seed: 11,
            water_level: 1.0,
        },
        materials,
    )
    .unwrap();
    world.paint(110, 80, 9, rock, Brush::Replace).unwrap();
    for (x, h) in [(20, 14), (26, 9), (130, 12)] {
        world
            .fill_rect(
                desktank_sim::Rect {
                    x0: x,
                    y0: 88 - h,
                    x1: x + 3,
                    y1: 88,
                },
                leaf,
                Brush::Replace,
            )
            .unwrap();
    }
    let names: Vec<String> = world.species().iter().map(|s| s.name.clone()).collect();
    for (i, name) in (0..8_i64).zip(&names) {
        let sp = world.species().id(name).unwrap();
        world
            .add_creature(sp, 20 + 16 * i, 22 + 9 * (i % 3))
            .unwrap();
    }
    let mut tank = Tank::new(world, TankConfig::default());
    tank.set_saver_mode(true);
    tank
}

#[test]
fn a_touring_tank_draws_the_tours_view_and_card_without_being_told_the_size() {
    let Some(gpu) = gpu() else { return };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = (640, 360);
    let settings = desktank_render::TourSettings {
        seed: 4,
        card: true,
    };
    let mut plain = tour_tank();
    let plain_image = render_sized(gpu, &mut plain, format, target);

    let mut tank = tour_tank();
    tank.set_tour(Some(settings));
    assert_eq!(tank.camera(), None, "no size known yet");
    // The first draw tells it the size, and the camera is on that frame (the
    // full-tank view, before the tour's first step).
    render_sized(gpu, &mut tank, format, target);
    assert_eq!(tank.camera(), Some(tank.camera_fit(target)));

    // Step until a glide has settled: the card is drawn beside its creature.
    let mut card = None;
    for _ in 0..60 * 20 {
        tank.step();
        card = tank.tour_card();
        if card.is_some() {
            break;
        }
    }
    let card = card.expect("a focus settled");
    assert_ne!(
        tank.camera(),
        Some(tank.camera_fit(target)),
        "the view has moved in"
    );
    assert_eq!(
        tank.inspected(),
        Some(desktank_render::Inspected::Creature(card.creature))
    );
    let with_card = render_sized(gpu, &mut tank, format, target);
    let at = tank.inspect_layout(target).expect("the card has a place");
    assert!(at.origin.0 + at.size().0 <= target.0 && at.origin.1 + at.size().1 <= target.1);
    // Without the card setting the same view draws without it.
    tank.set_tour(Some(desktank_render::TourSettings {
        card: false,
        ..settings
    }));
    assert_eq!(tank.inspected(), None);
    let without_card = render_sized(gpu, &mut tank, format, target);
    assert!(with_card != without_card, "the card is in the picture");

    // Turning the tour off draws the plain tank again, byte for byte.
    let mut off = tour_tank();
    off.set_tour(Some(settings));
    render_sized(gpu, &mut off, format, target);
    off.set_tour(None);
    assert_eq!(render_sized(gpu, &mut off, format, target), plain_image);
}

/// Writes a frame sequence of the tour (the card on) to `target/shots/tour/`
/// when `DESKTANK_SHOTS` is set: 12 frames, 15 s of world time apart.
#[test]
fn tour_screenshots() {
    let Some(gpu) = gpu() else { return };
    if std::env::var_os("DESKTANK_SHOTS").is_none() {
        return;
    }
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = (1280, 720);
    let mut tank = tour_tank();
    tank.set_target_size(target);
    tank.set_tour(Some(desktank_render::TourSettings {
        seed: 7,
        card: true,
    }));
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots/tour");
    std::fs::create_dir_all(&dir).unwrap();
    for frame in 0..12 {
        for _ in 0..15 * 20 {
            tank.step();
        }
        let img = render_sized(gpu, &mut tank, format, target);
        let file = std::fs::File::create(dir.join(format!("tour_{frame:02}.png"))).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), target.0, target.1);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&img).unwrap();
        eprintln!(
            "frame {frame}: {} card {:?} view {:?}",
            (frame + 1) * 15,
            tank.tour_card().map(|c| c.page),
            tank.camera()
        );
    }
}
