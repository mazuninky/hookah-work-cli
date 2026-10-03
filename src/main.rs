//! `hw` entry point: SIGPIPE reset, argument parsing, logging, dispatch to the library.

use std::io::IsTerminal;

use clap::Parser;
use tracing::Level;
use tracing_subscriber::EnvFilter;

use hw::cli::Cli;
use hw::error::{exit_code_for_clap_error, exit_code_for_error};
use hw::output::terminal_text;

fn main() {
    reset_sigpipe();
    let cli = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(err) => {
            // Help and version go to stdout with exit 0; usage errors exit 5, not clap's 2.
            let _ = err.print();
            std::process::exit(exit_code_for_clap_error(&err));
        }
    };
    init_logging(&cli);
    if let Err(err) = hw::app::run_system(&cli) {
        // Messages may quote API data; escape it like console output.
        eprintln!("Error: {}", terminal_text(&format!("{err:#}")));
        std::process::exit(exit_code_for_error(&err));
    }
}

/// Restores the default SIGPIPE disposition so `hw ... | head` exits quietly.
#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: runs first in `main`, before any thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

/// `-q` → ERROR, default WARN, `-v` INFO, `-vv` DEBUG, `-vvv` TRACE; `RUST_LOG` overrides.
fn init_logging(cli: &Cli) {
    let level = if cli.quiet {
        Level::ERROR
    } else {
        match cli.verbose {
            0 => Level::WARN,
            1 => Level::INFO,
            2 => Level::DEBUG,
            _ => Level::TRACE,
        }
    };
    let mut filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();
    // ureq logs the raw wire (Authorization included): off even under RUST_LOG=trace.
    for directive in ["ureq=off", "ureq_proto=off"] {
        if let Ok(directive) = directive.parse() {
            filter = filter.add_directive(directive);
        }
    }
    let ansi =
        !cli.no_color && std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(ansi)
        .try_init();
}
