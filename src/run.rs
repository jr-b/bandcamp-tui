//! The event loop: turns terminal input, API results and player events into [`Action`]s, feeds
//! them to [`App::update`], executes the resulting [`Effect`]s, and redraws only when something
//! changed.

use std::time::Duration;

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::MissedTickBehavior;

use crate::app::{Action, App, Effect, PlayerCommand};
use crate::bandcamp::{BandcampClient, DiscoverQuery};
use crate::format::error_chain;
use crate::input;
use crate::player::{PlayerEvent, PlayerHandle};

/// How often the progress bar updates while playing. Nothing ticks when paused or idle.
const PROGRESS_TICK: Duration = Duration::from_millis(250);

pub struct Services {
    pub client: BandcampClient,
    pub player: Option<PlayerHandle>,
    pub player_events: UnboundedReceiver<PlayerEvent>,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("player", &self.player.is_some())
            .finish_non_exhaustive()
    }
}

pub async fn run(
    terminal: &mut DefaultTerminal,
    services: Services,
    query: DiscoverQuery,
) -> std::io::Result<()> {
    let Services {
        client,
        player,
        mut player_events,
    } = services;
    let (results_tx, mut results) = unbounded_channel();
    let (mut app, effects) = App::new(query, player.is_some());
    let executor = Executor {
        client,
        player: player.clone(),
        results: results_tx,
    };
    executor.run(effects);

    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(PROGRESS_TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|frame| crate::ui::draw(frame, &mut app))?;
            dirty = false;
        }
        let action = tokio::select! {
            event = input.next() => match event {
                Some(Ok(Event::Key(key))) => input::action_for(key),
                Some(Ok(Event::Resize(..))) => Some(Action::Resize),
                Some(Ok(_)) => None,
                Some(Err(e)) => return Err(e),
                None => break, // stdin closed
            },
            Some(action) = results.recv() => Some(action),
            Some(event) = player_events.recv() => Some(Action::Player {
                event: Some(event),
                status: player.as_ref().map(PlayerHandle::status).unwrap_or_default(),
            }),
            _ = ticker.tick(), if app.is_playing() => player.as_ref().and_then(|p| {
                // Skip the redraw unless something visible changed (the clock shows whole
                // seconds), which cuts redraws from 4/s to 1/s while playing.
                let status = p.status();
                visibly_changed(&app.player, &status).then_some(Action::Player {
                    event: None,
                    status,
                })
            }),
        };
        if let Some(action) = action {
            let effects = app.update(action);
            executor.run(effects);
            dirty = true;
        }
        if app.should_quit {
            break;
        }
    }
    if let Some(player) = &player {
        player.stop();
    }
    Ok(())
}

fn visibly_changed(old: &crate::player::Status, new: &crate::player::Status) -> bool {
    old.state != new.state
        || old.index != new.index
        || old.duration != new.duration
        || old.position.as_secs() != new.position.as_secs()
}

struct Executor {
    client: BandcampClient,
    player: Option<PlayerHandle>,
    results: UnboundedSender<Action>,
}

impl Executor {
    fn run(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::FetchDiscover {
                    query,
                    cursor,
                    generation,
                } => {
                    let client = self.client.clone();
                    let tx = self.results.clone();
                    tokio::spawn(async move {
                        let result = client
                            .discover(&query, cursor.as_ref())
                            .await
                            .map_err(|e| error_chain(&e));
                        let _ = tx.send(Action::DiscoverLoaded { generation, result });
                    });
                }
                Effect::FetchAlbum { band_id, id, kind } => {
                    let client = self.client.clone();
                    let tx = self.results.clone();
                    tokio::spawn(async move {
                        let result = client
                            .album(band_id, id, kind)
                            .await
                            .map(Box::new)
                            .map_err(|e| error_chain(&e));
                        let _ = tx.send(Action::AlbumLoaded { id, result });
                    });
                }
                Effect::Player(command) => {
                    if let Some(player) = &self.player {
                        apply(player, command);
                    }
                }
            }
        }
    }
}

fn apply(player: &PlayerHandle, command: PlayerCommand) {
    match command {
        PlayerCommand::PlayQueue(queue, start) => player.play_queue(queue, start),
        PlayerCommand::TogglePause => player.toggle_pause(),
        PlayerCommand::Next => player.next(),
        PlayerCommand::Previous => player.previous(),
        PlayerCommand::SeekBy(ms) => player.seek_by(ms),
        PlayerCommand::SetVolume(v) => player.set_volume(v),
    }
}
