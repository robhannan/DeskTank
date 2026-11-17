//! DeskTank screensaver host: the `.scr` on Windows and an XScreenSaver hack on
//! X11. Started by the OS, not by users, so it takes the OS's own arguments
//! (`/s`, `/p HWND`, `/c`, `-window-id`) rather than a `clap` CLI.

use std::process::ExitCode;

// Dependency edge from SPEC §4.1, kept ahead of first use.
use desktank_platform as _;

fn main() -> ExitCode {
    eprintln!("desktank-saver: not yet implemented");
    ExitCode::FAILURE
}
