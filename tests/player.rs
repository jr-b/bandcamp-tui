//! Streaming source and player tests against a mock CDN that honours `Range` requests.
//! Audio goes to an in-memory rodio mixer, so no sound card is needed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Seek, SeekFrom};
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bandcamp_tui::player::{
    HttpSource, Output, PlaybackState, PlayerConfig, PlayerEvent, PlayerHandle, QueueTrack,
    StreamConfig, UrlRefresher,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedReceiver;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TONE: &[u8] = include_bytes!("fixtures/tone_8s.mp3");
const TONE_DURATION: Duration = Duration::from_secs(8);

/// Serves a file with HTTP range support and records the start offset of each request.
#[derive(Clone)]
struct RangeFile {
    data: &'static [u8],
    starts: Arc<Mutex<Vec<u64>>>,
}

impl RangeFile {
    fn new(data: &'static [u8]) -> Self {
        Self {
            data,
            starts: Arc::default(),
        }
    }

    fn starts(&self) -> Vec<u64> {
        self.starts.lock().unwrap().clone()
    }
}

impl Respond for RangeFile {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let len = self.data.len();
        let start: usize = request
            .headers
            .get("range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes="))
            .and_then(|v| v.strip_suffix('-'))
            .map_or(0, |v| v.parse().unwrap());
        self.starts.lock().unwrap().push(start as u64);
        if start >= len {
            return ResponseTemplate::new(416);
        }
        ResponseTemplate::new(206)
            .insert_header("content-range", format!("bytes {start}-{}/{len}", len - 1))
            .set_body_raw(self.data[start..].to_vec(), "audio/mpeg")
    }
}

struct Cdn {
    rt: Runtime,
    server: MockServer,
}

impl Cdn {
    fn start() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let server = rt.block_on(MockServer::start());
        Self { rt, server }
    }

    fn serve(&self, route: &str, responder: impl Respond + 'static) -> Url {
        self.rt.block_on(
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(responder)
                .mount(&self.server),
        );
        format!("{}{route}", self.server.uri()).parse().unwrap()
    }

    fn open(&self, url: Url, config: StreamConfig) -> std::io::Result<HttpSource> {
        HttpSource::open(
            self.rt.handle(),
            reqwest::Client::new(),
            url,
            config,
            None,
            |_| {},
        )
    }
}

/// A small window so tests exercise trimming, backpressure and out-of-window seeks.
const SMALL: StreamConfig = StreamConfig {
    buffer_bytes: 16 * 1024,
    back_bytes: 4 * 1024,
    forward_slack_bytes: 4 * 1024,
};

// ---- HttpSource --------------------------------------------------------------------------------

#[test]
fn reads_whole_file_within_memory_cap() {
    let cdn = Cdn::start();
    let file = RangeFile::new(TONE);
    let url = cdn.serve("/tone.mp3", file.clone());

    let mut source = cdn.open(url, SMALL).unwrap();
    assert_eq!(source.len(), TONE.len() as u64);
    let mut out = Vec::new();
    // Read slowly in small pieces, like a decoder does.
    let mut buf = [0u8; 4096];
    loop {
        let n = source.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    assert_eq!(out, TONE, "streamed bytes differ from the file");
    assert!(
        source.peak_buffered() <= SMALL.buffer_bytes,
        "buffered {} bytes, cap is {}",
        source.peak_buffered(),
        SMALL.buffer_bytes
    );
    assert_eq!(file.starts(), [0], "expected a single request");
}

#[test]
fn seeks_inside_and_outside_the_window() {
    let cdn = Cdn::start();
    let file = RangeFile::new(TONE);
    let url = cdn.serve("/tone.mp3", file.clone());
    let mut source = cdn.open(url, SMALL).unwrap();

    let mut buf = [0u8; 1000];
    source.read_exact(&mut buf).unwrap();
    source.seek(SeekFrom::Start(2000)).unwrap();
    source.read_exact(&mut buf).unwrap();
    assert_eq!(buf, TONE[2000..3000]);

    // Backwards within `back_bytes`: served from memory.
    source.seek(SeekFrom::Current(-1500)).unwrap();
    source.read_exact(&mut buf).unwrap();
    assert_eq!(buf, TONE[1500..2500]);
    assert_eq!(file.starts(), [0]);

    // Far forward: reconnect at the new offset.
    source.seek(SeekFrom::Start(100_000)).unwrap();
    source.read_exact(&mut buf).unwrap();
    assert_eq!(buf, TONE[100_000..101_000]);
    assert_eq!(file.starts(), [0, 100_000]);

    // Far backwards, out of the window again.
    source.seek(SeekFrom::Start(10)).unwrap();
    source.read_exact(&mut buf).unwrap();
    assert_eq!(buf, TONE[10..1010]);
    assert_eq!(file.starts(), [0, 100_000, 10]);

    // Read to the end after seeking near it.
    source.seek(SeekFrom::End(-5)).unwrap();
    let mut tail = Vec::new();
    source.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, TONE[TONE.len() - 5..]);
}

#[test]
fn expired_url_fails_with_permission_denied() {
    let cdn = Cdn::start();
    let url = cdn.serve("/expired.mp3", ResponseTemplate::new(403));
    let err = cdn.open(url, SMALL).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn expired_url_is_refreshed_and_the_download_continues() {
    let cdn = Cdn::start();
    let expired = cdn.serve("/expired.mp3", ResponseTemplate::new(403));
    let fresh = cdn.serve("/fresh.mp3", RangeFile::new(TONE));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let refresher: UrlRefresher = {
        let asked = Arc::clone(&asked);
        Arc::new(move |url: Url| {
            asked.lock().unwrap().push(url);
            let fresh = fresh.clone();
            Box::pin(async move { Some(fresh) })
        })
    };
    let mut source = HttpSource::open(
        cdn.rt.handle(),
        reqwest::Client::new(),
        expired.clone(),
        SMALL,
        Some(refresher),
        |_| {},
    )
    .unwrap();
    let mut buf = [0u8; 100];
    source.read_exact(&mut buf).unwrap();
    assert_eq!(buf, TONE[..100]);
    assert_eq!(*asked.lock().unwrap(), [expired]);
}

#[test]
fn a_refresher_that_cannot_help_fails_once() {
    let cdn = Cdn::start();
    let expired = cdn.serve("/expired.mp3", ResponseTemplate::new(410));
    let calls = Arc::new(Mutex::new(0));
    let refresher: UrlRefresher = {
        let calls = Arc::clone(&calls);
        Arc::new(move |_| {
            *calls.lock().unwrap() += 1;
            Box::pin(async { None })
        })
    };
    let err = HttpSource::open(
        cdn.rt.handle(),
        reqwest::Client::new(),
        expired,
        SMALL,
        Some(refresher),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[test]
fn retries_after_a_server_error() {
    let cdn = Cdn::start();
    cdn.rt.block_on(
        Mock::given(path("/flaky.mp3"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&cdn.server),
    );
    let url = cdn.serve("/flaky.mp3", RangeFile::new(TONE));
    let mut source = cdn.open(url, SMALL).unwrap();
    let mut buf = [0u8; 100];
    source.read_exact(&mut buf).unwrap();
    assert_eq!(buf, TONE[..100]);
}

#[test]
fn closer_aborts_a_stalled_open() {
    let cdn = Cdn::start();
    let url = cdn.serve(
        "/slow.mp3",
        ResponseTemplate::new(206).set_delay(Duration::from_secs(30)),
    );
    let started = Instant::now();
    let result = HttpSource::open(
        cdn.rt.handle(),
        reqwest::Client::new(),
        url,
        SMALL,
        None,
        |closer| {
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                closer.close();
            });
        },
    );
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    assert!(started.elapsed() < Duration::from_secs(5));
}

// ---- Player ------------------------------------------------------------------------------------

const RATE: NonZero<u32> = NonZero::<u32>::new(44_100).unwrap();
const CHANNELS: NonZero<u16> = NonZero::<u16>::new(2).unwrap();

/// Pulls audio from the mixer like a sound card would, but `speed` times faster.
struct FakeDevice {
    running: Arc<AtomicBool>,
    loud_samples: Arc<Mutex<u64>>,
}

impl FakeDevice {
    fn start(speed: u32) -> (Self, rodio::mixer::Mixer) {
        let (mixer, mut source) = rodio::mixer::mixer(CHANNELS, RATE);
        let running = Arc::new(AtomicBool::new(true));
        let loud_samples = Arc::new(Mutex::new(0));
        let (r, l) = (Arc::clone(&running), Arc::clone(&loud_samples));
        thread::spawn(move || {
            let per_tick = (44_100 * 2 * speed / 100) as usize; // 10 ms of audio
            while r.load(Ordering::Relaxed) {
                // The fixture peaks at 0.084; count samples above half of that.
                let loud = (&mut source)
                    .take(per_tick)
                    .filter(|s| s.abs() > 0.04)
                    .count() as u64;
                *l.lock().unwrap() += loud;
                thread::sleep(Duration::from_millis(10));
            }
        });
        (
            Self {
                running,
                loud_samples,
            },
            mixer,
        )
    }

    fn loud_samples(&self) -> u64 {
        *self.loud_samples.lock().unwrap()
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

struct Harness {
    cdn: Cdn,
    device: FakeDevice,
    player: PlayerHandle,
    events: UnboundedReceiver<PlayerEvent>,
}

impl Harness {
    fn new(speed: u32) -> Self {
        let cdn = Cdn::start();
        let (device, mixer) = FakeDevice::start(speed);
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let config = PlayerConfig {
            stream: SMALL,
            ..PlayerConfig::default()
        };
        let player = PlayerHandle::spawn(
            cdn.rt.handle().clone(),
            reqwest::Client::new(),
            Output::Mixer(mixer),
            config,
            None,
            tx,
        )
        .unwrap();
        Self {
            cdn,
            device,
            player,
            events,
        }
    }

    fn track(&self, route: &str, file: RangeFile) -> QueueTrack {
        QueueTrack {
            url: Some(self.cdn.serve(route, file)),
            duration: TONE_DURATION,
        }
    }

    /// Waits for an event matching `pred`, returning everything seen up to and including it.
    fn wait_for(&mut self, what: &str, pred: impl Fn(&PlayerEvent) -> bool) -> Vec<PlayerEvent> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            match self.events.try_recv() {
                Ok(event) => {
                    let done = pred(&event);
                    seen.push(event);
                    if done {
                        return seen;
                    }
                }
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("timed out waiting for {what}; saw {seen:?}");
    }

    fn wait_until(&self, what: &str, pred: impl Fn(&bandcamp_tui::player::Status) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if pred(&self.player.status()) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "timed out waiting for {what}; status {:?}",
            self.player.status()
        );
    }
}

#[test]
fn plays_a_queue_through_to_the_end() {
    let mut h = Harness::new(20);
    let first = RangeFile::new(TONE);
    let second = RangeFile::new(TONE);
    let queue = vec![
        h.track("/1.mp3", first.clone()),
        h.track("/2.mp3", second.clone()),
    ];
    h.player.play_queue(queue, 0);

    let events = h.wait_for("QueueEnded", |e| *e == PlayerEvent::QueueEnded);
    let tracks: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::TrackChanged(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(tracks, [Some(0), Some(1), None], "events: {events:?}");
    assert!(events.contains(&PlayerEvent::StateChanged(PlaybackState::Playing)));
    assert_eq!(first.starts(), [0]);
    assert_eq!(second.starts(), [0]);

    // Two 8 s stereo sine tones: about two thirds of samples exceed half the peak.
    let expected = 2 * 8 * 44_100 * 2;
    let loud = h.device.loud_samples();
    assert!(
        loud > expected / 2,
        "only {loud} loud samples of ~{expected}: audio isn't getting through"
    );
}

#[test]
fn skips_unplayable_and_failing_tracks() {
    let mut h = Harness::new(20);
    let unplayable = QueueTrack {
        url: None,
        duration: TONE_DURATION,
    };
    let broken = QueueTrack {
        url: Some(h.cdn.serve("/gone.mp3", ResponseTemplate::new(404))),
        duration: TONE_DURATION,
    };
    let good = h.track("/ok.mp3", RangeFile::new(TONE));
    h.player.play_queue(vec![unplayable, broken, good], 0);

    let events = h.wait_for("track 2", |e| *e == PlayerEvent::TrackChanged(Some(2)));
    assert_eq!(events[0], PlayerEvent::TrackChanged(Some(1)), "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PlayerEvent::TrackFailed { index: 1, .. })),
        "{events:?}"
    );
}

#[test]
fn expired_track_stops_the_queue_instead_of_skipping() {
    let mut h = Harness::new(20);
    let expired = QueueTrack {
        url: Some(h.cdn.serve("/old.mp3", ResponseTemplate::new(403))),
        duration: TONE_DURATION,
    };
    let next = h.track("/next.mp3", RangeFile::new(TONE));
    h.player.play_queue(vec![expired, next], 0);

    let events = h.wait_for("TrackFailed", |e| {
        matches!(e, PlayerEvent::TrackFailed { .. })
    });
    assert!(
        matches!(
            events.last(),
            Some(PlayerEvent::TrackFailed {
                index: 0,
                expired: true,
                ..
            })
        ),
        "{events:?}"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(h.player.status().state, PlaybackState::Stopped);
    assert!(
        !std::iter::from_fn(|| h.events.try_recv().ok())
            .any(|e| e == PlayerEvent::TrackChanged(Some(1))),
        "should not have moved on to track 1"
    );
}

#[test]
fn seeks_with_a_range_request() {
    // Real-time speed, so the track doesn't end before we seek.
    let h = Harness::new(1);
    let file = RangeFile::new(TONE);
    h.player
        .play_queue(vec![h.track("/seek.mp3", file.clone())], 0);
    h.wait_until("playing", |s| s.state == PlaybackState::Playing);

    h.player.seek(Duration::from_secs(6));
    h.wait_until("position past 6 s", |s| {
        s.state == PlaybackState::Playing && s.position >= Duration::from_secs(6)
    });
    let position = h.player.status().position;
    assert!(position < Duration::from_secs(7), "landed at {position:?}");

    // 6 s into a 128 kbps file is ~96 KB, far outside the 16 KiB window.
    let starts = file.starts();
    assert_eq!(starts.len(), 2, "{starts:?}");
    let expected = 6 * 128_000 / 8;
    assert!(
        starts[1].abs_diff(expected) < 8_000,
        "seek requested byte {} (expected ~{expected})",
        starts[1]
    );
}

#[test]
fn pause_freezes_position() {
    let h = Harness::new(1);
    h.player
        .play_queue(vec![h.track("/p.mp3", RangeFile::new(TONE))], 0);
    h.wait_until("some progress", |s| s.position > Duration::from_millis(300));

    h.player.pause();
    h.wait_until("paused", |s| s.state == PlaybackState::Paused);
    let before = h.player.status().position;
    thread::sleep(Duration::from_millis(400));
    assert_eq!(h.player.status().position, before);

    h.player.resume();
    h.wait_until("playing again", |s| s.position > before);
}
