//! `desktank bench`: a scripted scenario with performance numbers
//! (SPEC §6.1). A seeded sand pour into the starter scene, then the tank
//! settles; rendering goes to an offscreen 2560×1440 target at the default
//! 4× cell scale, paced in real time exactly like the window.
//!
//! GPU timing reads timestamps back after each frame, which blocks that
//! frame on the GPU (waiting costs little CPU). "CPU per frame" is process
//! CPU time across the tick and the draw. The settled phase draws no frames,
//! so its numbers are the production path.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use desktank_platform::{ProcessStats, process_stats};
use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, wgpu};
use desktank_sim::{Brush, MaterialTable, World, WorldConfig};

use crate::scene;

/// Reference display (SPEC §6.1): 2560×1440 at 4× = 640×360 cells.
const TARGET: (u32, u32) = (2560, 1440);
const CELL_SCALE: u32 = 4;
const SEED: u64 = 7;
/// Simulation ticks of pouring (5 s of world time).
const POUR_TICKS: u64 = 100;
const POUR_RADIUS: u32 = 4;

/// Runs the scenario for `seconds` of wall time and prints the report.
///
/// # Errors
///
/// Returns an error if no GPU is available or the world can't be built.
pub fn run(seconds: u64) -> anyhow::Result<()> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let gpu = pollster::block_on(Gpu::new(&instance, None, GpuOptions { timestamps: true }))?;
    let config = WorldConfig {
        width: (TARGET.0 / CELL_SCALE) as usize,
        height: (TARGET.1 / CELL_SCALE) as usize,
        seed: SEED,
        ..WorldConfig::default()
    };
    let mut world = World::new(config, MaterialTable::builtin()?)?;
    scene::starter(&mut world, SEED)?;
    let sand = scene::material(&world, "sand")?;
    let mut tank = Tank::new(world, TankConfig::default());
    let gpu_timing = tank.enable_gpu_timing(&gpu);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let view = offscreen_target(&gpu, format);

    let duration = Duration::from_secs(seconds);
    let start = Instant::now();
    let mut active = Phase::new(start, process_stats());
    let mut settled: Option<Phase> = None;
    let mut gpu_times = Samples::default();
    let mut max_awake = 0;
    // The last tick poured into, so each tick gets exactly one disc.
    let mut poured = None;
    loop {
        let now = start.elapsed();
        if now >= duration {
            break;
        }
        // The scripted pour: a disc of sand each tick, sweeping sideways.
        let world = tank.world();
        let tick = world.tick_count();
        if tick < POUR_TICKS && poured != Some(tick) {
            poured = Some(tick);
            let w = i64::try_from(world.width()).unwrap_or(0);
            let x = w / 2 + i64::try_from(tick).unwrap_or(0) * 2 - 100;
            let y = i64::try_from(world.surface_row()).unwrap_or(0) + 10;
            tank.world_mut()
                .paint(x, y, POUR_RADIUS, sand, Brush::Pour)?;
        }
        let cpu0 = process_stats().cpu_time;
        let t0 = Instant::now();
        let req = tank.tick(now);
        let tick_time = t0.elapsed();
        let phase = settled.as_mut().unwrap_or(&mut active);
        if req.sim_ticks > 0 {
            phase.ticks += req.sim_ticks;
            phase.tick_times.push(tick_time / req.sim_ticks);
        }
        if req.redraw {
            tank.render_to(&gpu, &view, format, TARGET);
            phase.frames += 1;
            phase
                .frame_cpu
                .push(process_stats().cpu_time.saturating_sub(cpu0));
            if let Some(t) = tank.last_gpu_time() {
                gpu_times.push(t);
            }
        }
        let awake = tank.world().awake_chunks();
        max_awake = max_awake.max(awake);
        phase.max_awake = phase.max_awake.max(awake);
        if settled.is_none() && tank.world().tick_count() > POUR_TICKS && awake == 0 {
            let now = Instant::now();
            let stats = process_stats();
            active.end(now, stats);
            settled = Some(Phase::new(now, stats));
        }
        if let Some(wait) = req.wake_at.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    let end = (Instant::now(), process_stats());
    match settled.as_mut() {
        Some(s) => s.end(end.0, end.1),
        None => active.end(end.0, end.1),
    }
    let report = Report {
        seconds,
        start,
        active: &active,
        settled: settled.as_ref(),
        gpu_times: gpu_timing.then_some(&gpu_times),
        max_awake,
        end: end.1,
    };
    print!("{}", report.render(&gpu, tank.world()));
    Ok(())
}

/// The offscreen 2560×1440 target frames are drawn into.
fn offscreen_target(gpu: &Gpu, format: wgpu::TextureFormat) -> wgpu::TextureView {
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width: TARGET.0,
            height: TARGET.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    target.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Everything the printed report needs.
struct Report<'a> {
    seconds: u64,
    start: Instant,
    active: &'a Phase,
    settled: Option<&'a Phase>,
    /// `None` if the adapter has no timestamp queries.
    gpu_times: Option<&'a Samples>,
    max_awake: usize,
    end: ProcessStats,
}

impl Report<'_> {
    fn render(&self, gpu: &Gpu, world: &World) -> String {
        let Self {
            seconds,
            start,
            active,
            settled,
            max_awake,
            ..
        } = *self;
        let info = gpu.adapter.get_info();
        let mut out = String::new();
        let _ = writeln!(
            out,
            "desktank bench: {seconds} s, {}x{} cells -> {}x{} offscreen, seed {SEED}, {} ({:?})",
            world.width(),
            world.height(),
            TARGET.0,
            TARGET.1,
            info.name,
            info.backend,
        );
        let settle_at = settled.map(|s| s.start.duration_since(start));
        active.report(&mut out, "pour + settle", start);
        if let Some(s) = settled {
            s.report(&mut out, "settled", start);
        }
        let _ = writeln!(
            out,
            "settled after: {}",
            settle_at.map_or("never (still awake at the end)".into(), |t| format!(
                "{:.1} s",
                t.as_secs_f64()
            ))
        );
        let _ = writeln!(
            out,
            "awake chunks: max {max_awake} of {}, at end {}",
            world.chunk_count(),
            world.awake_chunks()
        );
        match self.gpu_times {
            Some(t) => {
                let _ = writeln!(out, "GPU frame time ({} frames): {}", t.len(), t.summary());
            }
            None => {
                let _ = writeln!(
                    out,
                    "GPU frame time: unavailable (adapter has no timestamp queries)"
                );
            }
        }
        let mb = |b: Option<u64>| {
            b.map_or("n/a".into(), |b| {
                #[allow(clippy::cast_precision_loss)]
                let m = b as f64 / f64::from(1 << 20);
                format!("{m:.1} MB")
            })
        };
        let _ = writeln!(
            out,
            "RSS: {} now, {} peak",
            mb(self.end.rss_bytes),
            mb(self.end.peak_rss_bytes)
        );
        out
    }
}

/// Measurements over one stretch of the run.
struct Phase {
    start: Instant,
    start_stats: ProcessStats,
    wall: Duration,
    cpu: Duration,
    ticks: u32,
    frames: u32,
    max_awake: usize,
    tick_times: Samples,
    frame_cpu: Samples,
}

impl Phase {
    fn new(start: Instant, stats: ProcessStats) -> Self {
        Self {
            start,
            start_stats: stats,
            wall: Duration::ZERO,
            cpu: Duration::ZERO,
            ticks: 0,
            frames: 0,
            max_awake: 0,
            tick_times: Samples::default(),
            frame_cpu: Samples::default(),
        }
    }

    fn end(&mut self, at: Instant, stats: ProcessStats) {
        self.wall = at.duration_since(self.start);
        self.cpu = stats.cpu_time.saturating_sub(self.start_stats.cpu_time);
    }

    fn report(&self, out: &mut String, name: &str, origin: Instant) {
        let from = self.start.duration_since(origin).as_secs_f64();
        let pct = if self.wall.is_zero() {
            0.0
        } else {
            100.0 * self.cpu.as_secs_f64() / self.wall.as_secs_f64()
        };
        let _ = writeln!(
            out,
            "{name} ({from:.1}–{:.1} s): CPU {pct:.2}% of a core, {} ticks, {} frames, max {} awake chunks",
            from + self.wall.as_secs_f64(),
            self.ticks,
            self.frames,
            self.max_awake,
        );
        let _ = writeln!(out, "  sim tick: {}", self.tick_times.summary());
        let _ = writeln!(out, "  CPU per frame: {}", self.frame_cpu.summary());
    }
}

/// Durations with a mean / p95 / max summary.
#[derive(Default)]
struct Samples(Vec<Duration>);

impl Samples {
    fn push(&mut self, d: Duration) {
        self.0.push(d);
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn summary(&self) -> String {
        if self.0.is_empty() {
            return "no samples".into();
        }
        let mut v = self.0.clone();
        v.sort_unstable();
        let n = u32::try_from(v.len()).unwrap_or(u32::MAX);
        let mean = v.iter().sum::<Duration>() / n;
        let p95 = v[(v.len() * 95 / 100).min(v.len() - 1)];
        let max = v[v.len() - 1];
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        format!(
            "mean {:.3} ms, p95 {:.3} ms, max {:.3} ms",
            ms(mean),
            ms(p95),
            ms(max)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_samples() {
        let mut s = Samples::default();
        assert_eq!(s.summary(), "no samples");
        for ms in 1..=20 {
            s.push(Duration::from_millis(ms));
        }
        assert_eq!(s.summary(), "mean 10.500 ms, p95 20.000 ms, max 20.000 ms");
    }
}
