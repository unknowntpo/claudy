mod app;
mod message;
mod session;
mod ui;
mod watcher;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "claudy", about = "Claude Code Session Monitor TUI")]
struct Cli {
    /// Path to Claude projects directory
    #[arg(short, long)]
    path: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let base_path = cli.path.unwrap_or_else(|| {
        let home = dirs::home_dir().expect("Could not determine home directory");
        home.join(".claude").join("projects")
    });

    if !base_path.exists() {
        eprintln!("Claude projects directory not found: {}", base_path.display());
        eprintln!("Make sure Claude Code is installed and has been used at least once.");
        std::process::exit(1);
    }

    let mut app = app::App::new(base_path)?;

    let mut terminal = ratatui::init();
    let result = app.run_event_loop(&mut terminal);
    ratatui::restore();

    result
}
