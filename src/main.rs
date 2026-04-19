use std::path::PathBuf;

use clap::{Parser, Subcommand};
use hyprcrop::commands::capture;
use hyprcrop::domain::config::Config;
use hyprcrop::domain::error::{AppError, Result};
use hyprcrop::platform::system::{clipboard, lock, notify};
use hyprcrop::ui::freeze;

#[derive(Parser)]
#[command(name = "hyprcrop", about = "Hyprland screenshot tool", version)]
struct Cli {
    /// Path to a custom config file (defaults to ~/.config/hyprcrop/config.toml)
    #[arg(long, short, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Select a region with slurp and capture it
    Crop,
    /// Capture the active window (geometry via hyprctl)
    Window,
    /// Capture the window via xdg-desktop-portal (not yet implemented)
    Portal,
    /// Capture the focused monitor
    Monitor,
    /// Capture all monitors
    All,
    /// Freeze screen and select region interactively
    Freeze,
    /// Write a default config.toml to ~/.config/hyprcrop/config.toml (or --config path)
    GenerateConfig {
        /// Overwrite the file if it already exists
        #[arg(long)]
        force: bool,
    },
}

fn run(cli: &Cli, cfg: &Config) -> Result<()> {
    // Create save directory during initialization
    std::fs::create_dir_all(&cfg.save_path)
        .map_err(|e| AppError::FileSystem(cfg.save_path.clone(), e))?;

    match cli.command {
        Commands::Crop => finish(capture::capture_crop(cfg)?, cfg)?,
        Commands::Window => finish(capture::capture_window(cfg)?, cfg)?,
        Commands::Portal => finish(capture::capture_portal(cfg)?, cfg)?,
        Commands::Monitor => finish(capture::capture_monitor(cfg)?, cfg)?,
        Commands::All => finish(capture::capture_all(cfg)?, cfg)?,
        Commands::Freeze => {
            let path = {
                let _lock = lock::FreezeLock::acquire()?;
                freeze::run_freeze(cfg)?
            };
            finish(path, cfg)?
        }
        Commands::GenerateConfig { .. } => unreachable!(),
    }

    Ok(())
}

fn generate_config(custom_path: Option<&std::path::Path>, force: bool) -> Result<()> {
    let path = custom_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(Config::default_config_path);

    if path.exists() && !force {
        return Err(AppError::Config(format!(
            "config file already exists: {}\nUse --force to overwrite",
            path.display()
        )));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::FileSystem(parent.to_path_buf(), e))?;
    }

    let content = Config::generate_default_toml()?;
    std::fs::write(&path, &content).map_err(|e| AppError::FileSystem(path.clone(), e))?;

    println!("config written to: {}", path.display());
    Ok(())
}

fn finish(path: std::path::PathBuf, cfg: &Config) -> Result<()> {
    clipboard::copy_to_clipboard(&path)?;
    println!("{}", path.display());
    notify::notify_success(&path, &cfg.notifications);
    Ok(())
}

fn main() {
    let cli = Cli::parse();

    if let Commands::GenerateConfig { force } = cli.command {
        if let Err(e) = generate_config(cli.config.as_deref(), force) {
            eprintln!("error: {}", e);
            std::process::exit(e.exit_code());
        }
        return;
    }

    let cfg = match &cli.config {
        Some(path) => match Config::load_from(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: failed to load config '{}': {}", path.display(), e);
                std::process::exit(e.exit_code());
            }
        },
        None => match Config::load() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: failed to load config: {}", e);
                std::process::exit(e.exit_code());
            }
        },
    };

    if let Err(e) = run(&cli, &cfg) {
        if let AppError::UserCancelled = &e {
            std::process::exit(e.exit_code());
        }

        eprintln!("error: {}", e);
        notify::notify_error(&e.to_string(), &cfg.notifications);
        std::process::exit(e.exit_code());
    }
}
