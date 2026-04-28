#![warn(clippy::cognitive_complexity)]
#![warn(clippy::too_many_lines)]

mod app;
mod backend;
mod config;
mod inspector;
mod types;
mod ui;

use anyhow::Result;

fn print_help() {
    println!(
        "\
mtp-tui — TUI file manager for MTP devices

Usage: mtp-tui [OPTIONS]

Options:
  -h, --help     Print this help message and exit
  -V, --version  Print version and exit"
    );
}

fn main() -> Result<()> {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "-V" | "--version" => {
                println!("mtp-tui {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown option '{other}'");
                eprintln!();
                print_help();
                std::process::exit(1);
            }
            _ => {}
        }
    }

    let terminal = ratatui::init();
    let result = app::App::new().and_then(|app| app.run(terminal));
    ratatui::restore();
    result
}
