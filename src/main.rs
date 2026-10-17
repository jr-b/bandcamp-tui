//! bandcamp-tui: browse Bandcamp Discover and stream full albums from the terminal.

use std::fs::File;
use std::path::PathBuf;

use bandcamp_tui::bandcamp::{BandcampClient, Slice};
use bandcamp_tui::player::{Output, PlayerConfig, PlayerHandle};
use bandcamp_tui::run::{Services, run};
use bandcamp_tui::settings::Settings;
use clap::{Parser, ValueEnum};
use color_eyre::eyre::{Result, WrapErr};
use tracing_subscriber::EnvFilter;

/// Browse Bandcamp Discover and stream full albums. No account needed.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Genre or tag to browse, e.g. `electronic`. Repeat to narrow down (`-t electronic -t techno`).
    /// Defaults to the last one used.
    #[arg(short, long = "tag", value_name = "TAG")]
    tags: Vec<String>,

    /// Sort order. Defaults to the last one used.
    #[arg(short, long, value_enum)]
    slice: Option<SliceArg>,

    /// Start with default filters and volume instead of the remembered ones.
    #[arg(long)]
    fresh: bool,

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
    let data_dir = directories::ProjectDirs::from("", "", "bandcamp-tui")
        .map(|dirs| dirs.data_local_dir().to_path_buf());
    init_logging(cli.log_file, data_dir.as_deref())?;

    let settings_path = data_dir.map(|dir| dir.join("state.toml"));
    let settings = match &settings_path {
        Some(path) if !cli.fresh => Settings::load(path),
        _ => Settings::default(),
    };
    let mut query = settings.query();
    if !cli.tags.is_empty() {
        query.tags = cli.tags;
    }
    if let Some(slice) = cli.slice {
        query.slice = slice.into();
    }

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
        Ok(player) => {
            player.set_volume(settings.volume);
            Some(player)
        }
        Err(e) => {
            tracing::error!(error = %bandcamp_tui::format::error_chain(&e), "no audio output");
            None
        }
    };

    let services = Services {
        client,
        player,
        player_events,
        settings_path,
        initial_volume: settings.volume,
    };

    // Installs a panic hook that restores the terminal before the color-eyre report prints.
    let mut terminal = ratatui::init();
    let result = runtime.block_on(run(&mut terminal, services, query));
    ratatui::restore();
    result.wrap_err("terminal I/O failed")
}

/// Logs go to a file: stdout and stderr belong to the TUI.
fn init_logging(path: Option<PathBuf>, data_dir: Option<&std::path::Path>) -> Result<()> {
    let path = if let Some(path) = path {
        path
    } else {
        let dir = data_dir.ok_or_else(|| color_eyre::eyre::eyre!("no home directory"))?;
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
