mod app;
mod clipboard;
mod config;
mod daemon;
mod ipc;
mod store;
mod touchpad;
mod ui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "clipbord",
    about = "Clipboard history for Fedora / GNOME — Win+V style",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Headless background daemon (no GTK, no dock icon). Default.
    Daemon,
    /// Open the history popup (short-lived UI).
    Toggle,
    /// Same as toggle.
    Show,
    /// Clear non-pinned history via the daemon.
    Clear,
    /// Print status and paths.
    Status,
    /// Print the default config path.
    ConfigPath,
    /// Toggle / control the laptop touchpad (for typing without palm hits).
    Touchpad {
        #[command(subcommand)]
        action: Option<TouchpadCmd>,
    },
}

#[derive(Subcommand, Debug)]
enum TouchpadCmd {
    /// Flip touchpad on ↔ off (default when no subcommand).
    Toggle,
    /// Enable the touchpad.
    #[command(alias = "enable")]
    On,
    /// Disable the touchpad (useful while typing).
    #[command(alias = "disable")]
    Off,
    /// Print current touchpad state.
    Status,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let config = config::Config::load().context("load config")?;

    match cli.command.unwrap_or(Commands::Daemon) {
        Commands::Daemon => daemon::run(config),
        Commands::Toggle | Commands::Show => app::run_ui(config),
        Commands::Clear => {
            ipc::request(&ipc::Request::Clear)?;
            Ok(())
        }
        Commands::Status => print_status(),
        Commands::ConfigPath => {
            println!("{}", config::Config::path()?.display());
            Ok(())
        }
        Commands::Touchpad { action } => run_touchpad(action),
    }
}

fn run_touchpad(action: Option<TouchpadCmd>) -> Result<()> {
    let state = match action.unwrap_or(TouchpadCmd::Toggle) {
        TouchpadCmd::Toggle => touchpad::toggle()?,
        TouchpadCmd::On => touchpad::set_enabled(true)?,
        TouchpadCmd::Off => touchpad::set_enabled(false)?,
        TouchpadCmd::Status => touchpad::status()?,
    };
    println!("touchpad: {}", state.label());
    Ok(())
}

fn print_status() -> Result<()> {
    let socket = ipc::socket_path()?;
    let db = store::db_path()?;
    let running = ipc::ping().is_ok();
    println!("clipbord {}", env!("CARGO_PKG_VERSION"));
    println!("config:   {}", config::Config::path()?.display());
    println!("socket:   {}", socket.display());
    println!("database: {}", db.display());
    println!("daemon:   {}", if running { "running (headless)" } else { "stopped" });
    if running {
        if let Ok(ipc::Response::Stats { count, bytes }) = ipc::request(&ipc::Request::Stats) {
            println!("history:  {count} items · {}", store::human_bytes(bytes));
        }
    }
    Ok(())
}
