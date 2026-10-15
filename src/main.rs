#![windows_subsystem = "windows"]

use std::env;
use std::path::PathBuf;
use std::time::Instant;

mod anim;
mod fonts;
mod gpu;
mod highlight;
mod layout;
mod parser;
mod renderer;
mod texmath;
mod theme;
mod window;

#[cfg(test)]
mod resilience_tests;

fn main() -> windows::core::Result<()> {
    let startup_timer = Instant::now();

    // 1. Resolve markdown target file from command line (double-click %1 in Explorer).
    //    With no argument the window falls back to ./README.md, then the built-in help.
    let target_file = env::args_os().nth(1).map(PathBuf::from);

    // 2. Launch native Win32 window with Direct2D rendering. Themes (embedded or
    //    custom JSON from %APPDATA%\WackDown\themes) are loaded by the window.
    window::run_window_loop(target_file, startup_timer)
}
