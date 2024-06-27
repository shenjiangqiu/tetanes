//! A NES Emulator written in Rust with `WebAssembly` support
//!
//! USAGE:
//!     tetanes [FLAGS] [OPTIONS] [path]
//!
//! FLAGS:
//!     -f, --fullscreen    Start fullscreen.
//!     -h, --help          Prints help information
//!     -V, --version       Prints version information
//!
//! OPTIONS:
//!     -s, --scale <scale>    Window scale [default: 3.0]
//!
//! ARGS:
//!     <path>    The NES ROM to load, a directory containing `.nes` ROM files, or a recording
//!               playback `.playback` file. [default: current directory]

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cfg_if::cfg_if;
use tetanes::{logging, nes::Nes};

#[cfg(not(target_arch = "wasm32"))]
mod opts;

fn main() -> anyhow::Result<()> {
    let log = logging::init();
    if let Err(err) = log {
        eprintln!("failed to initialize logging: {err:?}");
    }

    #[cfg(feature = "profiling")]
    puffin::set_scopes_on(true);

    Nes::run({
        cfg_if! {
            if #[cfg(target_arch = "wasm32")] {
                tetanes::nes::config::Config::load(None)
            } else {
                use clap::Parser;

                let opts = opts::Opts::parse();
                tracing::debug!("CLI Options: {opts:?}");

                opts.load()?
            }
        }
    })
}
