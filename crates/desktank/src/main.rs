//! DeskTank main binary: CLI, config, tray, IPC, event loop and mode
//! orchestration (SPEC §4.3).

use std::path::PathBuf;
use std::process::ExitCode;

mod bench;
mod capture;
mod config;
mod ipc;
mod overlay;
mod persist;
mod window;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// A lightweight pixel-art aquarium for your desktop.
#[derive(Debug, Parser)]
#[command(name = "desktank", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the long-running app in the chosen display modes.
    Run(RunArgs),
    /// Open the tank in a normal resizable window.
    Window(WindowArgs),
    /// Turn interact mode on or off in the running app.
    Interact {
        /// What to do with interact mode.
        state: InteractState,
    },
    /// Drop food into the tank of the running app.
    Feed {
        /// Food to drop; defaults to the standard flake food.
        food: Option<String>,
    },
    /// Pause or resume the running app.
    Pause,
    /// Report the running app's pause state, interact flag, frame cap and creature count.
    Status,
    /// Render frames headlessly to image files.
    Capture(CaptureArgs),
    /// Run a scripted scenario and print performance numbers.
    Bench(BenchArgs),
    /// Make a save file (from another machine, or exported from the web
    /// version) the tank's newest save. The app must not be running.
    Import {
        /// The `.dtank` file to import.
        file: PathBuf,
    },
    /// Copy the tank's newest save to a file (to import on the web version
    /// or another machine).
    Export {
        /// Where to write the `.dtank` file.
        file: PathBuf,
    },
    /// Install or uninstall the screensaver.
    Saver {
        #[command(subcommand)]
        action: SaverAction,
    },
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Draw the tank behind the desktop icons.
    #[arg(long)]
    wallpaper: bool,
    /// Float a transparent, click-through tank above open windows.
    #[arg(long)]
    overlay: bool,
    /// Show the tank when the machine is idle.
    #[arg(long)]
    idle: bool,
}

/// Parses a `--time-ratio` in `0.1..=100` (finite).
fn parse_time_ratio(s: &str) -> Result<f32, String> {
    let ratio: f32 = s.parse().map_err(|_| format!("{s:?} is not a number"))?;
    if ratio.is_finite()
        && (desktank_sim::MIN_TIME_RATIO..=desktank_sim::MAX_TIME_RATIO).contains(&ratio)
    {
        Ok(ratio)
    } else {
        Err(format!("{s:?} is not in 0.1..=100"))
    }
}

/// Parses a `--latitude` in degrees north, `-90..=90`.
fn parse_latitude(s: &str) -> Result<f64, String> {
    let degrees: f64 = s.parse().map_err(|_| format!("{s:?} is not a number"))?;
    if config::is_latitude(degrees) {
        Ok(degrees)
    } else {
        Err(format!("{s:?} is not in -90..=90"))
    }
}

/// Parses a `--longitude` in degrees east, `-180..=180`.
fn parse_longitude(s: &str) -> Result<f64, String> {
    let degrees: f64 = s.parse().map_err(|_| format!("{s:?} is not a number"))?;
    if config::is_longitude(degrees) {
        Ok(degrees)
    } else {
        Err(format!("{s:?} is not in -180..=180"))
    }
}

#[derive(Debug, Args)]
struct WindowArgs {
    /// Drive a scripted pour (a synthetic left drag sweeping across the
    /// tank) for this many seconds, then print the CPU it used.
    #[arg(long, value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..=600))]
    demo_pour: Option<u64>,
    /// Local time of day to light the tank for, as HH:MM (default 12:00).
    #[arg(long, value_name = "HH:MM", value_parser = capture::parse_time)]
    time: Option<u16>,
    /// Stock the tank with this many fish (every fish species in turn).
    #[arg(long, value_name = "N", default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=60))]
    fish: u16,
    /// Start a new tank instead of opening the saved one (the older saves
    /// are kept until newer ones replace them).
    #[arg(long, conflicts_with = "no_save")]
    fresh: bool,
    /// Start a new tank and don't save it.
    #[arg(long)]
    no_save: bool,
    /// Easy mode: creatures die only of old age (`--easy-mode=false` turns
    /// it off). Saved with the tank.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    easy_mode: Option<bool>,
    /// The population cap (default 60, at most 128; never below the
    /// creatures already in the tank). Saved with the tank.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=128))]
    creature_cap: Option<u16>,
    /// Apply elapsed time when the tank reopens or resumes (off by default).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    catch_up: Option<bool>,
    /// Life and day/night speed: 1 is real time, 60 passes a day in 24
    /// minutes (0.1 to 100). Saved with the tank.
    #[arg(long, value_name = "RATIO", value_parser = parse_time_ratio)]
    time_ratio: Option<f32>,
    /// Where sunrise and sunset come from: `fixed` (06:30 and 20:30) or
    /// `location` (the sun where you are). The window has no OS location, so
    /// `location` needs --latitude and --longitude.
    #[arg(long, value_name = "MODE", value_enum)]
    daylight_schedule: Option<config::DaylightMode>,
    /// Degrees north (-90 to 90) for the sun's schedule; needs --longitude.
    #[arg(long, value_name = "DEG", requires = "longitude", allow_negative_numbers = true, value_parser = parse_latitude)]
    latitude: Option<f64>,
    /// Degrees east (-180 to 180) for the sun's schedule; needs --latitude.
    #[arg(long, value_name = "DEG", requires = "latitude", allow_negative_numbers = true, value_parser = parse_longitude)]
    longitude: Option<f64>,
    /// OLED mode: true black water, air and letterbox (`--oled=false` turns
    /// it off when the config file enables it).
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    oled: Option<bool>,
    /// Draw the centimetre ruler overlay along the bottom and left edges
    /// (`U` toggles it in the window).
    #[arg(long)]
    ruler: bool,
    #[command(flatten)]
    plant_sway: SwayArgs,
}

#[derive(Debug, Args)]
struct SwayArgs {
    /// Optional render-only plant sway (10 fps cap; off by default).
    #[arg(long)]
    sway: bool,
    /// Allow plant sway on battery when plant sway is enabled.
    #[arg(long)]
    sway_on_battery: bool,
}

#[derive(Debug, Args)]
struct BenchArgs {
    /// Compare 24 hours of catch-up against full simulation without a GPU.
    #[arg(long, conflicts_with_all = ["seconds", "fish", "plants", "sway"])]
    catch_up: bool,
    /// Length of the run in seconds of wall time.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
    seconds: u64,
    /// Fish swimming in the tank (every fish species in turn; SPEC §6.1's
    /// reference tank has about 30).
    #[arg(long, value_name = "N", default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=60))]
    fish: u16,
    /// Pre-grow a planted bed before timing (for measuring plant rendering
    /// and optional sway).
    #[arg(long)]
    plants: bool,
    /// Enable optional plant sway during the benchmark (10 fps cap).
    #[arg(long)]
    sway: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum InteractState {
    /// Turn interact mode on.
    On,
    /// Turn interact mode off.
    Off,
    /// Switch interact mode to the other state.
    Toggle,
}

#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // independent command-line flags
struct CaptureArgs {
    /// World seed.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Local time of day to light the tank for, as HH:MM (default 12:00).
    #[arg(long, value_name = "HH:MM", value_parser = capture::parse_time)]
    time: Option<u16>,
    /// Output size in pixels, as width x height (for example 640x360).
    #[arg(long, value_name = "WxH", default_value = "640x360", value_parser = capture::parse_size)]
    size: (u32, u32),
    /// Number of frames to render, `frame_every` ticks apart (see the script).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10_000))]
    frames: u32,
    /// Input script of timed pours (TOML; see docs/capture.md).
    #[arg(long, value_name = "FILE")]
    script: Option<PathBuf>,
    /// Tint the tank by a field (debug overlay): nutrient, co2, light or flow.
    #[arg(long, value_name = "FIELD", value_parser = capture::parse_field)]
    field: Option<desktank_render::FieldOverlay>,
    /// Enable optional render-only plant sway.
    #[arg(long)]
    sway: bool,
    /// OLED mode: true black water, air and letterbox.
    #[arg(long)]
    oled: bool,
    /// Draw the centimetre ruler overlay along the bottom and left edges.
    #[arg(long)]
    ruler: bool,
    /// Draw with the split grid (smaller shaders in more passes), as the
    /// webOS web build does.
    #[arg(long)]
    split_grid: bool,
    /// Directory to write frames to (`frame.png`, or `frame_NNNN.png` for several).
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
}

#[derive(Debug, Clone, Copy, Subcommand)]
enum SaverAction {
    /// Install the screensaver for the current user.
    Install,
    /// Remove the installed screensaver.
    Uninstall,
}

impl Command {
    /// The subcommand as typed on the command line, for messages.
    fn name(&self) -> &'static str {
        match self {
            Self::Run(_) => "run",
            Self::Window(_) => "window",
            Self::Interact { .. } => "interact",
            Self::Feed { .. } => "feed",
            Self::Pause => "pause",
            Self::Status => "status",
            Self::Capture(_) => "capture",
            Self::Bench(_) => "bench",
            Self::Import { .. } => "import",
            Self::Export { .. } => "export",
            Self::Saver {
                action: SaverAction::Install,
            } => "saver install",
            Self::Saver {
                action: SaverAction::Uninstall,
            } => "saver uninstall",
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Run(args) if args.wallpaper || args.idle => Err(anyhow::anyhow!(
            "--wallpaper and --idle are not yet implemented"
        )),
        Command::Run(args) if args.overlay => overlay::run(),
        Command::Run(_) => Err(anyhow::anyhow!("specify --overlay")),
        Command::Window(args) => window::run(
            args.demo_pour,
            args.time,
            usize::from(args.fish),
            args.ruler,
            if args.no_save {
                window::Saves::Off
            } else if args.fresh {
                window::Saves::Fresh
            } else {
                window::Saves::Load
            },
            config::CliOverrides {
                easy_mode: args.easy_mode,
                population_cap: args.creature_cap,
                catch_up: args.catch_up,
                time_ratio: args.time_ratio,
                daylight_schedule: args.daylight_schedule,
                latitude: args.latitude,
                longitude: args.longitude,
                oled: args.oled,
                plants: config::Plants {
                    sway: args.plant_sway.sway,
                    sway_on_battery: args.plant_sway.sway_on_battery,
                },
            },
        ),
        Command::Bench(args) if args.catch_up => bench::run_catch_up(),
        Command::Interact { state } => send_ipc(&format!(
            "interact {}",
            match state {
                InteractState::On => "on",
                InteractState::Off => "off",
                InteractState::Toggle => "toggle",
            }
        )),
        Command::Feed { food: None } => send_ipc("feed"),
        Command::Feed { food: Some(food) } => {
            if food.is_empty() || food.bytes().any(|b| b.is_ascii_whitespace()) {
                Err(ipc::ClientError::InvalidRequest.into())
            } else {
                send_ipc(&format!("feed {food}"))
            }
        }
        Command::Pause => send_ipc("pause"),
        Command::Status => send_ipc("status"),
        Command::Bench(args) => {
            bench::run(args.seconds, usize::from(args.fish), args.plants, args.sway)
        }
        Command::Import { file } => persist::import(None, file),
        Command::Export { file } => persist::export(None, file),
        Command::Capture(args) => capture::run(&capture::Options {
            seed: args.seed,
            time: args.time,
            size: args.size,
            frames: args.frames,
            script: args.script.clone(),
            field: args.field,
            sway: args.sway,
            oled: args.oled,
            ruler: args.ruler,
            split_grid: args.split_grid,
            out: args.out.clone(),
        }),
        other => {
            eprintln!("desktank {}: not yet implemented", other.name());
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("desktank {}: {e:#}", cli.command.name());
            ExitCode::FAILURE
        }
    }
}

fn send_ipc(request: &str) -> anyhow::Result<()> {
    println!("{}", ipc::send(request)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn window_sway_preferences_are_independent_and_off_by_default() {
        let window = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "window"].iter().chain(args)).unwrap();
            let Command::Window(w) = cli.command else {
                panic!("expected a window command");
            };
            (w.plant_sway.sway, w.plant_sway.sway_on_battery)
        };
        assert_eq!(window(&[]), (false, false));
        assert_eq!(window(&["--sway"]), (true, false));
        assert_eq!(window(&["--sway-on-battery"]), (false, true));
        assert_eq!(window(&["--sway", "--sway-on-battery"]), (true, true));
    }

    #[test]
    fn parses_spec_examples() {
        let cases: &[(&[&str], &str)] = &[
            (&["run", "--wallpaper", "--overlay", "--idle"], "run"),
            (&["window"], "window"),
            (&["window", "--demo-pour", "10"], "window"),
            (&["window", "--time", "21:00"], "window"),
            (&["window", "--fresh"], "window"),
            (&["window", "--no-save"], "window"),
            (&["window", "--easy-mode"], "window"),
            (
                &["window", "--easy-mode=false", "--creature-cap", "30"],
                "window",
            ),
            (&["interact", "toggle"], "interact"),
            (&["feed"], "feed"),
            (&["feed", "sinking_pellet"], "feed"),
            (&["pause"], "pause"),
            (&["status"], "status"),
            (
                &[
                    "capture",
                    "--seed",
                    "7",
                    "--time",
                    "12:00",
                    "--size",
                    "640x360",
                    "--frames",
                    "3",
                    "--script",
                    "pour.toml",
                    "--field",
                    "nutrient",
                    "--out",
                    "shots",
                ],
                "capture",
            ),
            (&["bench"], "bench"),
            (&["bench", "--catch-up"], "bench"),
            (&["bench", "--seconds", "5", "--plants", "--sway"], "bench"),
            (&["import", "tank.dtank"], "import"),
            (&["export", "tank.dtank"], "export"),
            (&["saver", "install"], "saver install"),
            (&["saver", "uninstall"], "saver uninstall"),
        ];
        for (args, name) in cases {
            let cli = Cli::try_parse_from(std::iter::once("desktank").chain(args.iter().copied()))
                .unwrap_or_else(|e| panic!("{args:?}: {e}"));
            assert_eq!(cli.command.name(), *name);
        }
    }

    #[test]
    fn parses_the_life_cycle_settings() {
        let window = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "window"].iter().chain(args)).unwrap();
            let Command::Window(w) = cli.command else {
                panic!("not a window command");
            };
            (w.easy_mode, w.creature_cap, w.catch_up)
        };
        assert_eq!(window(&[]), (None, None, None));
        assert_eq!(window(&["--easy-mode"]), (Some(true), None, None));
        assert_eq!(window(&["--catch-up"]), (None, None, Some(true)));
        assert_eq!(
            window(&[
                "--easy-mode=false",
                "--creature-cap",
                "30",
                "--catch-up=false"
            ]),
            (Some(false), Some(30), Some(false))
        );
        assert!(Cli::try_parse_from(["desktank", "window", "--creature-cap", "129"]).is_err());
        assert!(Cli::try_parse_from(["desktank", "window", "--creature-cap", "0"]).is_err());
    }

    #[test]
    fn parses_the_time_ratio() {
        let ratio = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "window"].iter().chain(args)).unwrap();
            let Command::Window(w) = cli.command else {
                panic!("not a window command");
            };
            w.time_ratio
        };
        assert_eq!(ratio(&[]), None);
        assert_eq!(ratio(&["--time-ratio", "60"]), Some(60.0));
        assert_eq!(ratio(&["--time-ratio=0.1"]), Some(0.1));
        assert_eq!(ratio(&["--time-ratio", "100"]), Some(100.0));
        for bad in ["0", "0.09", "-1", "100.1", "nan", "inf", "fast", ""] {
            assert!(
                Cli::try_parse_from(["desktank", "window", "--time-ratio", bad]).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn parses_the_oled_flag() {
        let oled = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "window"].iter().chain(args)).unwrap();
            let Command::Window(w) = cli.command else {
                panic!("not a window command");
            };
            w.oled
        };
        assert_eq!(oled(&[]), None);
        assert_eq!(oled(&["--oled"]), Some(true));
        assert_eq!(oled(&["--oled=false"]), Some(false));
        assert!(Cli::try_parse_from(["desktank", "capture", "--out", "x", "--oled"]).is_ok());
    }

    #[test]
    fn parses_the_ruler_flag() {
        let window = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "window"].iter().chain(args)).unwrap();
            let Command::Window(w) = cli.command else {
                panic!("not a window command");
            };
            w.ruler
        };
        assert!(!window(&[]));
        assert!(window(&["--ruler"]));
        let capture = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "capture", "--out", "x"].iter().chain(args))
                .unwrap();
            let Command::Capture(c) = cli.command else {
                panic!("not a capture command");
            };
            c.ruler
        };
        assert!(!capture(&[]));
        assert!(capture(&["--ruler"]));
    }

    #[test]
    fn parses_the_daylight_settings() {
        let daylight = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "window"].iter().chain(args)).unwrap();
            let Command::Window(w) = cli.command else {
                panic!("not a window command");
            };
            (w.daylight_schedule, w.latitude, w.longitude)
        };
        assert_eq!(daylight(&[]), (None, None, None));
        assert_eq!(
            daylight(&["--daylight-schedule", "fixed"]),
            (Some(config::DaylightMode::Fixed), None, None)
        );
        assert_eq!(
            daylight(&[
                "--daylight-schedule=location",
                "--latitude",
                "51.5",
                "--longitude=0.0",
                "--time",
                "16:30",
            ]),
            (Some(config::DaylightMode::Location), Some(51.5), Some(0.0))
        );
        assert_eq!(
            daylight(&["--latitude", "-33.9", "--longitude", "-180"]),
            (None, Some(-33.9), Some(-180.0))
        );
        assert_eq!(
            daylight(&["--latitude=90", "--longitude=180"]),
            (None, Some(90.0), Some(180.0))
        );
    }

    #[test]
    fn rejects_bad_daylight_settings() {
        let bad =
            |args: &[&str]| Cli::try_parse_from(["desktank", "window"].iter().chain(args)).is_err();
        assert!(bad(&["--daylight-schedule", "sunny"]));
        assert!(bad(&["--daylight-schedule", "Location"]));
        assert!(bad(&["--latitude", "51.5"]), "longitude is missing");
        assert!(bad(&["--longitude", "0"]), "latitude is missing");
        for degrees in ["90.1", "-90.1", "nan", "inf", "north", ""] {
            assert!(
                bad(&["--latitude", degrees, "--longitude", "0"]),
                "{degrees:?}"
            );
        }
        for degrees in ["180.1", "-180.1", "nan", "-inf", "east", ""] {
            assert!(
                bad(&["--latitude", "0", "--longitude", degrees]),
                "{degrees:?}"
            );
        }
    }

    #[test]
    fn rejects_unknown_interact_state() {
        assert!(Cli::try_parse_from(["desktank", "interact", "maybe"]).is_err());
    }
}
