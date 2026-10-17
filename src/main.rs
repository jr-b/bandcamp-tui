//! bandcamp-tui: browse Bandcamp Discover and stream full albums from the terminal.

use std::fs::File;
use std::path::PathBuf;

use bandcamp_tui::bandcamp::{BandcampClient, DiscoverQuery, Slice};
use bandcamp_tui::player::{Output, PlayerConfig, PlayerHandle};
use bandcamp_tui::run::{Services, run};
use clap::{Parser, ValueEnum};
use color_eyre::eyre::{Result, WrapErr};
use tracing_subscriber::EnvFilter;

/// Browse Bandcamp Discover and stream full albums. No account needed.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Genre or tag to browse, e.g. `electronic`. Repeat to narrow down (`-t electronic -t techno`).
    #[arg(short, long = "tag", value_name = "TAG")]
    tags: Vec<String>,

    /// Sort order.
    #[arg(short, long, value_enum, default_value_t = SliceArg::Top)]
    slice: SliceArg,

    /// Log file. Defaults to `bandcamp-tui.log` in the platform data directory
    /// (`~/Library/Application Support/bandcamp-tui` on macOS, `~/.local/share/bandcamp-tui` on Linux).
    #[arg(long, value_name = "PATH")]
    log_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SliceArg {
    /// Best-selling.
    Top,
    /// New arrivals.
    New,
    /// Surprise me.
    Random,
}

impl From<SliceArg> for Slice {
    fn from(s: SliceArg) -> Self {
        match s {
            SliceArg::Top => Self::Top,
            SliceArg::New => Self::New,
            SliceArg::Random => Self::Random,
        }
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    init_logging(cli.log_file)?;

    // The UI runs on the main thread; one worker handles network I/O.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .wrap_err("starting async runtime")?;
    let client = BandcampClient::new().wrap_err("creating HTTP client")?;

    let (player_tx, player_events) = tokio::sync::mpsc::unbounded_channel();
    let player = match PlayerHandle::spawn(
        runtime.handle().clone(),
        client.http().clone(),
        Output::DefaultDevice,
        PlayerConfig::default(),
        player_tx,
    ) {
        Ok(player) => Some(player),
        Err(e) => {
            tracing::error!(error = %bandcamp_tui::format::error_chain(&e), "no audio output");
            None
        }
    };

    let query = DiscoverQuery {
        tags: cli.tags,
        slice: cli.slice.into(),
        ..DiscoverQuery::default()
    };
    let services = Services {
        client,
        player,
        player_events,
    };

    // Installs a panic hook that restores the terminal before the color-eyre report prints.
    let mut terminal = ratatui::init();
    let result = runtime.block_on(run(&mut terminal, services, query));
    ratatui::restore();
    result.wrap_err("terminal I/O failed")
}

/// Logs go to a file: stdout and stderr belong to the TUI.
fn init_logging(path: Option<PathBuf>) -> Result<()> {
    let path = if let Some(path) = path {
        path
    } else {
        let dirs = directories::ProjectDirs::from("", "", "bandcamp-tui")
            .ok_or_else(|| color_eyre::eyre::eyre!("no home directory"))?;
        let dir = dirs.data_local_dir();
        std::fs::create_dir_all(dir).wrap_err_with(|| format!("creating {}", dir.display()))?;
        dir.join("bandcamp-tui.log")
    };
    let file = File::create(&path).wrap_err_with(|| format!("creating {}", path.display()))?;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("bandcamp_tui=info,symphonia=error,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .init();
    tracing::info!(path = %path.display(), "logging started");
    Ok(())
}
