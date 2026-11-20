//! DeskTank main binary: CLI, config, tray, IPC, event loop and mode
//! orchestration (SPEC §4.3).

use std::path::PathBuf;
use std::process::ExitCode;

mod bench;
mod capture;
mod persist;
mod window;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Default screen pixels per simulation cell.
const DEFAULT_CELL_SCALE: u32 = 2;

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
    /// Screen pixels per simulation cell for a new tank (1..=8; odd values
    /// above 1 round down to even). A saved tank keeps its own grid size.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_CELL_SCALE, value_parser = clap::value_parser!(u32).range(1..=8))]
    scale: u32,
}

#[derive(Debug, Args)]
struct BenchArgs {
    /// Length of the run in seconds of wall time.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
    seconds: u64,
    /// Fish swimming in the tank (every fish species in turn; SPEC §6.1's
    /// reference tank has about 30).
    #[arg(long, value_name = "N", default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=60))]
    fish: u16,
    /// Screen pixels per simulation cell (1..=8; odd values above 1 round
    /// down to even).
    #[arg(long, value_name = "N", default_value_t = DEFAULT_CELL_SCALE, value_parser = clap::value_parser!(u32).range(1..=8))]
    scale: u32,
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
        Command::Window(args) => window::run(
            args.demo_pour,
            args.time,
            usize::from(args.fish),
            if args.no_save {
                window::Saves::Off
            } else if args.fresh {
                window::Saves::Fresh
            } else {
                window::Saves::Load
            },
            window::LifeSettings {
                easy: args.easy_mode,
                cap: args.creature_cap.map(usize::from),
            },
            desktank_render::even_scale(args.scale),
        ),
        Command::Bench(args) => bench::run(
            args.seconds,
            usize::from(args.fish),
            desktank_render::even_scale(args.scale),
        ),
        Command::Import { file } => persist::import(None, file),
        Command::Export { file } => persist::export(None, file),
        Command::Capture(args) => capture::run(&capture::Options {
            seed: args.seed,
            time: args.time,
            size: args.size,
            frames: args.frames,
            script: args.script.clone(),
            field: args.field,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
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
            (&["feed", "pellets"], "feed"),
            (&["pause"], "pause"),
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
            (&["bench", "--seconds", "5"], "bench"),
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
            (w.easy_mode, w.creature_cap)
        };
        assert_eq!(window(&[]), (None, None));
        assert_eq!(window(&["--easy-mode"]), (Some(true), None));
        assert_eq!(
            window(&["--easy-mode=false", "--creature-cap", "30"]),
            (Some(false), Some(30))
        );
        assert!(Cli::try_parse_from(["desktank", "window", "--creature-cap", "129"]).is_err());
        assert!(Cli::try_parse_from(["desktank", "window", "--creature-cap", "0"]).is_err());
    }

    #[test]
    fn parses_scale_for_window_and_bench() {
        let win_scale = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "window"].iter().chain(args)).unwrap();
            let Command::Window(w) = cli.command else {
                panic!("not a window command");
            };
            desktank_render::even_scale(w.scale)
        };
        assert_eq!(win_scale(&[]), 2);
        assert_eq!(win_scale(&["--scale", "4"]), 4);
        assert_eq!(win_scale(&["--scale", "3"]), 2);
        assert_eq!(win_scale(&["--scale", "1"]), 1);
        assert_eq!(win_scale(&["--scale", "8"]), 8);
        assert!(Cli::try_parse_from(["desktank", "window", "--scale", "0"]).is_err());
        assert!(Cli::try_parse_from(["desktank", "window", "--scale", "9"]).is_err());

        let bench_scale = |args: &[&str]| {
            let cli = Cli::try_parse_from(["desktank", "bench"].iter().chain(args)).unwrap();
            let Command::Bench(b) = cli.command else {
                panic!("not a bench command");
            };
            desktank_render::even_scale(b.scale)
        };
        assert_eq!(bench_scale(&[]), 2);
        assert_eq!(bench_scale(&["--scale", "4"]), 4);
        assert_eq!(bench_scale(&["--scale", "5"]), 4);
        assert!(Cli::try_parse_from(["desktank", "bench", "--scale", "0"]).is_err());
        assert!(Cli::try_parse_from(["desktank", "bench", "--scale", "9"]).is_err());
    }

    #[test]
    fn rejects_unknown_interact_state() {
        assert!(Cli::try_parse_from(["desktank", "interact", "maybe"]).is_err());
    }
}
