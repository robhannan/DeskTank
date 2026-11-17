//! DeskTank main binary: CLI, config, tray, IPC, event loop and mode
//! orchestration (SPEC §4.3).

use std::path::PathBuf;
use std::process::ExitCode;

mod bench;
mod scene;
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
    Window,
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
struct BenchArgs {
    /// Length of the run in seconds of wall time.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
    seconds: u64,
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
    #[arg(long)]
    seed: Option<u64>,
    /// Local time of day in the tank, as HH:MM.
    #[arg(long, value_name = "HH:MM")]
    time: Option<String>,
    /// Output size in pixels, as width x height (for example 640x360).
    #[arg(long, value_name = "WxH")]
    size: Option<String>,
    /// Number of frames to render.
    #[arg(long)]
    frames: Option<u32>,
    /// Script of pours and other inputs to replay.
    #[arg(long, value_name = "FILE")]
    script: Option<PathBuf>,
    /// Directory to write frames to.
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
            Self::Window => "window",
            Self::Interact { .. } => "interact",
            Self::Feed { .. } => "feed",
            Self::Pause => "pause",
            Self::Capture(_) => "capture",
            Self::Bench(_) => "bench",
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
        Command::Window => window::run(),
        Command::Bench(args) => bench::run(args.seconds),
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
                    "--out",
                    "shots",
                ],
                "capture",
            ),
            (&["bench"], "bench"),
            (&["bench", "--seconds", "5"], "bench"),
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
    fn rejects_unknown_interact_state() {
        assert!(Cli::try_parse_from(["desktank", "interact", "maybe"]).is_err());
    }
}
