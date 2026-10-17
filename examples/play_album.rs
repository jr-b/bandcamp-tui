//! Streams the first playable album from Bandcamp Discover through the speakers, end to end
//! without a UI.
//!
//! ```sh
//! cargo run --release --example play_album -- [--tag electronic] [--seconds 30] [--seek-test]
//! ```
//!
//! `--seek-test` exercises seeking (forward and back out of the buffer window) and skipping.
//! Set `RUST_LOG=bandcamp_tui=debug` for pipeline logs.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::time::Duration;

use bandcamp_tui::bandcamp::{BandcampClient, DiscoverQuery, Slice};
use bandcamp_tui::player::{Output, PlayerConfig, PlayerEvent, PlayerHandle, QueueTrack};

struct Args {
    tag: Option<String>,
    seconds: u64,
    seek_test: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        tag: None,
        seconds: 30,
        seek_test: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tag" => args.tag = it.next(),
            "--seconds" => args.seconds = it.next().and_then(|s| s.parse().ok()).unwrap_or(30),
            "--seek-test" => args.seek_test = true,
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    args
}

fn fmt(d: Duration) -> String {
    format!("{}:{:02}", d.as_secs() / 60, d.as_secs() % 60)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args = parse_args();

    // One worker thread is plenty: it only shuffles bytes.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let client = BandcampClient::new()?;

    let query = DiscoverQuery {
        tags: args.tag.into_iter().collect(),
        slice: Slice::Top,
        page_size: 20,
        ..DiscoverQuery::default()
    };
    let album = runtime.block_on(async {
        let page = client.discover(&query, None).await?;
        for item in page.items.iter().filter(|i| !i.is_preorder) {
            let album = client.album(item.band_id, item.id, item.kind).await?;
            if album.has_playable_tracks() {
                return Ok(album);
            }
        }
        Err::<_, Box<dyn std::error::Error>>("no playable album on the first page".into())
    })?;

    println!("▶ {} — {}", album.title, album.artist);
    println!("  {}", album.url);
    for t in &album.tracks {
        let mark = if t.is_playable() { " " } else { "✗" };
        println!(
            "  {mark}{:>2}. {} ({})",
            t.number.unwrap_or(0),
            t.title,
            fmt(t.duration)
        );
    }

    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let player = PlayerHandle::spawn(
        runtime.handle().clone(),
        client.http().clone(),
        Output::DefaultDevice,
        PlayerConfig::default(),
        events_tx,
    )?;
    player.set_volume(0.5);
    player.play_queue(album.tracks.iter().map(QueueTrack::from).collect(), 0);

    runtime.block_on(async {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        let started = tokio::time::Instant::now();
        let mut step = 0;
        loop {
            tokio::select! {
                Some(event) = events.recv() => {
                    println!("  event: {event:?}");
                    if event == PlayerEvent::QueueEnded {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    let elapsed = started.elapsed().as_secs();
                    let s = player.status();
                    let title = s.index.map_or("-", |i| album.tracks[i].title.as_str());
                    println!(
                        "  [{elapsed:>3}s] {:?} #{} {title} {} / {}",
                        s.state,
                        s.index.map_or(0, |i| i + 1),
                        fmt(s.position),
                        fmt(s.duration)
                    );
                    if args.seek_test {
                        let target = match (step, elapsed) {
                            (0, 6..) => Some(("seek to 60%", Some(s.duration.mul_f32(0.6)))),
                            (1, 11..) => Some(("seek back to 0:10", Some(Duration::from_secs(10)))),
                            (2, 16..) => Some(("next track", None)),
                            _ => None,
                        };
                        if let Some((label, to)) = target {
                            println!("  >>> {label}");
                            match to {
                                Some(to) => player.seek(to),
                                None => player.next(),
                            }
                            step += 1;
                        }
                    }
                    if elapsed >= args.seconds {
                        break;
                    }
                }
            }
        }
    });
    player.stop();
    // Give the engine a moment to tear down cleanly before the runtime goes away.
    std::thread::sleep(Duration::from_millis(200));
    Ok(())
}
