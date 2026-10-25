//! bandcamp-tui: browse Bandcamp Discover and stream full albums from the terminal.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use bandcamp_tui::bandcamp::{BandcampClient, Slice};
use bandcamp_tui::config::{AlbumArt, Config};
use bandcamp_tui::player::{Output, PlayerConfig, PlayerHandle, UrlRefresher};
use bandcamp_tui::run::{Services, run};
use bandcamp_tui::settings::Settings;
use bandcamp_tui::ui::theme::{self, Theme};
use clap::{Parser, ValueEnum};
use color_eyre::eyre::{Result, WrapErr};
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
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

    /// Never show cover art (same as `album_art = "off"` in the config).
    #[arg(long)]
    no_art: bool,

    /// Config file. Defaults to `config.toml` in the platform config directory
    /// (`~/Library/Application Support/bandcamp-tui` on macOS, `~/.config/bandcamp-tui` on Linux).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

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
    let dirs = directories::ProjectDirs::from("", "", "bandcamp-tui");
    let data_dir = dirs.as_ref().map(|d| d.data_local_dir().to_path_buf());
    init_logging(cli.log_file, data_dir.as_deref())?;

    let config_path = cli
        .config
        .or_else(|| dirs.as_ref().map(|d| d.config_dir().join("config.toml")));
    let mut config = match &config_path {
        Some(path) => Config::load(path)?,
        None => Config::default(),
    };
    if cli.no_art {
        config.album_art = AlbumArt::Off;
    }

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

    let refresher: UrlRefresher = {
        let client = client.clone();
        Arc::new(move |url| {
            let client = client.clone();
            Box::pin(async move {
                client
                    .refresh_stream_url(&url)
                    .await
                    .inspect_err(|e| {
                        let error = bandcamp_tui::format::error_chain(e);
                        tracing::warn!(%error, "couldn't refresh stream URL");
                    })
                    .ok()
            })
        })
    };
    let (player_tx, player_events) = tokio::sync::mpsc::unbounded_channel();
    let player = match PlayerHandle::spawn(
        runtime.handle().clone(),
        client.http().clone(),
        Output::DefaultDevice,
        PlayerConfig::default(),
        Some(refresher),
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

    // Before raw mode: the query sets and restores it itself. `block_on` renders on this thread.
    theme::set(Theme::resolve(
        config.theme,
        &config.colors,
        theme::no_color(),
        theme::detect_background,
    ));

    // Installs a panic hook that restores the terminal before the color-eyre report prints.
    let mut terminal = ratatui::init();
    // Must run in the alternate screen and before input is read: it asks the terminal.
    let art = bandcamp_tui::art::detect(config.album_art);
    // After the capability queries, so no mouse report gets mixed into their answers.
    enable_mouse();
    let services = Services {
        client,
        player,
        player_events,
        settings_path,
        initial_volume: settings.volume,
        art: art.picker,
        art_note: art.note,
        autoplay: config.autoplay,
        listen_on_queue: config.listen_on_queue,
        density: config.density,
        chosen_density: settings.density,
        screen: settings.screen,
        cover_hidden: settings.cover_hidden,
        queue: settings.queue.clone(),
    };
    let result = runtime.block_on(run(&mut terminal, services, query));
    disable_mouse();
    ratatui::restore();
    result.wrap_err("terminal I/O failed")
}

/// Mouse reports for the wheel. `ratatui::restore` doesn't turn them off, so a panic hook does
/// it before ratatui's own (installed by `ratatui::init`) restores the rest.
fn enable_mouse() {
    if let Err(e) = execute!(std::io::stdout(), EnableMouseCapture) {
        tracing::warn!(error = %e, "couldn't enable the mouse");
        return;
    }
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        disable_mouse();
        restore(info);
    }));
}

fn disable_mouse() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
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
    // Keep the previous session's log (e.g. a crash to report) as `<name>.1`.
    let previous = path.with_file_name(format!(
        "{}.1",
        path.file_name()
            .map_or_else(Default::default, |n| n.to_string_lossy())
    ));
    let rotated = match std::fs::rename(&path, &previous) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
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
    if let Err(e) = rotated {
        tracing::warn!(path = %previous.display(), error = %e, "couldn't keep the previous log");
    }
    Ok(())
}
