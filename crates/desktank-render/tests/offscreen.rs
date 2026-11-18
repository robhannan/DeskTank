//! Smoke test: draw a tiny world offscreen and read pixels back. Skipped
//! (with a message) on machines without a GPU adapter, but fails when `CI`
//! is set: CI has Metal, WARP/DX12 and lavapipe. Reference-image tests are
//! in `crates/desktank/tests/render.rs`.

use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, wgpu};
use desktank_sim::{MaterialTable, World, WorldConfig};

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
    let size = wgpu::Extent3d {
        width: W,
        height: H,
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
    tank.render_to(gpu, &view, format, (W, H));
    let padded = (W * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * H),
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
        .flat_map(|row| &row[..(W * 4) as usize])
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
    // The rock cell covers pixels 20..24 × 20..24, in one of its shades.
    let r = px(&img, 21, 22);
    for (got, base) in r.iter().zip(rock_colour) {
        assert!(got.abs_diff(base) <= 16, "rock {r:?} vs {rock_colour:?}");
    }
    // Its 4×4 block is uniform (nearest, integer scale).
    for y in 20..24 {
        for x in 20..24 {
            assert_eq!(px(&img, x, y), r);
        }
    }
    // Water around it is blue.
    let [wr, _, wb] = px(&img, 9, 1);
    assert!(wb > wr);
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
    let leaf_colour = materials.get(leaf).unwrap().colour;
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
    for (got, base) in before.iter().zip(leaf_colour) {
        assert!(
            got.abs_diff(base) <= 24,
            "leaf {before:?} vs {leaf_colour:?}"
        );
    }
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
