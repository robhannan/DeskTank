//! `desktank capture`: render a scripted scene headlessly to PNG files
//! (SPEC §4.3, T3). No window: frames go to an offscreen target and are
//! read back. The input script format is documented in
//! [`desktank_sim::script`] and `docs/capture.md`.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, wgpu};
use desktank_sim::{MaterialTable, Script, World};

use crate::scene;

/// What to capture, from the command line.
#[derive(Debug)]
pub struct Options {
    /// World seed.
    pub seed: u64,
    /// Time of day as minutes after midnight. Day and night don't exist
    /// yet, so this is accepted and validated but changes nothing.
    pub time: Option<u16>,
    /// Output size in pixels.
    pub size: (u32, u32),
    /// Number of frames.
    pub frames: u32,
    /// Input script; `None` is the empty script (an idle default world).
    pub script: Option<PathBuf>,
    /// Output directory.
    pub out: PathBuf,
}

/// Runs the capture and prints each file written.
///
/// # Errors
///
/// Returns an error if the script is invalid, no GPU is available, or a
/// file can't be written.
pub fn run(opts: &Options) -> anyhow::Result<()> {
    let script = match &opts.script {
        Some(path) => {
            let src =
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            Script::parse(&src).with_context(|| format!("in {}", path.display()))?
        }
        None => Script::default(),
    };
    let _ = opts.time; // Passed through for day/night (T-later); no effect yet.
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let gpu = pollster::block_on(Gpu::new(&instance, None, GpuOptions::default()))?;

    let mut world = World::new(script.world_config(opts.seed), MaterialTable::builtin()?)?;
    if script.world.starter {
        scene::starter(&mut world, opts.seed)?;
    }
    let mut tank = Tank::new(world, TankConfig::default());
    fs::create_dir_all(&opts.out).with_context(|| format!("creating {}", opts.out.display()))?;

    let mut until = script.ticks;
    for frame in 0..opts.frames {
        while tank.world().tick_count() < until {
            let t = tank.world().tick_count();
            script.apply(tank.world_mut(), t)?;
            tank.step();
        }
        let pixels = tank.capture(&gpu, opts.size)?;
        let name = if opts.frames == 1 {
            "frame.png".to_owned()
        } else {
            format!("frame_{frame:04}.png")
        };
        let path = opts.out.join(name);
        write_png(&path, opts.size, &pixels)?;
        println!("{}", path.display());
        until += script.frame_every;
    }
    Ok(())
}

/// Writes RGBA8 `pixels` as an sRGB PNG.
fn write_png(path: &Path, (w, h): (u32, u32), pixels: &[u8]) -> anyhow::Result<()> {
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()?;
    Ok(())
}

/// Parses `WxH` (for example `640x360`).
pub fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("{s:?} is not WxH"))?;
    let w: u32 = w.parse().map_err(|_| format!("bad width in {s:?}"))?;
    let h: u32 = h.parse().map_err(|_| format!("bad height in {s:?}"))?;
    if w == 0 || h == 0 {
        return Err(format!("{s:?} has a zero side"));
    }
    Ok((w, h))
}

/// Parses `HH:MM` into minutes after midnight.
pub fn parse_time(s: &str) -> Result<u16, String> {
    let bad = || format!("{s:?} is not a time HH:MM");
    let (h, m) = s.split_once(':').ok_or_else(bad)?;
    let (h, m): (u16, u16) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if h > 23 || m > 59 || s.len() != 5 {
        return Err(bad());
    }
    Ok(h * 60 + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes_and_times() {
        assert_eq!(parse_size("640x360"), Ok((640, 360)));
        assert_eq!(parse_size("2X1"), Ok((2, 1)));
        for bad in ["640", "0x5", "ax5", "5x"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_time("07:30"), Ok(450));
        assert_eq!(parse_time("23:59"), Ok(1439));
        for bad in ["24:00", "7:30", "12:60", "noon"] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
    }
}
