//! A seekable, memory-bounded HTTP byte stream.
//!
//! A tokio task downloads the file into a fixed-size window. The decoder reads from it through
//! blocking [`Read`]/[`Seek`] calls on its own thread. Memory use is capped at
//! [`StreamConfig::buffer_bytes`], however long the file is:
//!
//! * The download pauses when the window is full and resumes as the reader consumes it, so we
//!   only download about as fast as we play.
//! * The reader keeps up to [`StreamConfig::back_bytes`] of already-read data for short backward
//!   seeks.
//! * A seek outside the window drops it and reconnects with an HTTP `Range` request at the new
//!   offset.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use tokio::sync::Notify;
use url::Url;

/// Gets a fresh URL for an expired one, or `None` if it can't. Called at most once per
/// expiry, from the download task.
pub type UrlRefresher =
    Arc<dyn Fn(Url) -> Pin<Box<dyn Future<Output = Option<Url>> + Send>> + Send + Sync>;

/// Sizes of the download window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfig {
    /// Hard cap on buffered bytes. 1 MiB holds about 64 s of 128 kbps audio.
    pub buffer_bytes: usize,
    /// How much already-read data to keep for backward seeks. Must be < `buffer_bytes`.
    pub back_bytes: usize,
    /// A forward seek this close past the downloaded data waits for it rather than reconnecting.
    pub forward_slack_bytes: u64,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            buffer_bytes: 1024 * 1024,
            back_bytes: 256 * 1024,
            forward_slack_bytes: 64 * 1024,
        }
    }
}

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);
/// Time allowed to receive response headers. Body reads have no timeout, because a full
/// window legitimately stalls the download for minutes while playback is paused.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The blocking reader half. Dropping it stops the download task.
#[derive(Debug)]
pub struct HttpSource {
    shared: Arc<Shared>,
    /// Absolute read position in the file.
    pos: u64,
    len: u64,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    /// Signals the reader: data arrived, error, or length known.
    data_ready: Condvar,
    /// Signals the writer: space freed, restart requested, or closed.
    writer_wake: Notify,
    config: StreamConfig,
}

#[derive(Debug)]
struct State {
    /// Bytes `[buf_start, buf_start + buf.len())` of the file.
    buf: VecDeque<u8>,
    buf_start: u64,
    /// Where the reader is; the writer trims data before `read_pos - back_bytes`.
    read_pos: u64,
    /// Bumped on every restart so the writer drops data from an outdated connection.
    generation: u64,
    content_length: Option<u64>,
    error: Option<io::ErrorKind>,
    error_message: Option<String>,
    closed: bool,
    /// Highest `buf.len()` seen; lets tests verify the memory cap.
    peak_buffered: usize,
}

impl State {
    fn buf_end(&self) -> u64 {
        self.buf_start + self.buf.len() as u64
    }

    fn restart_at(&mut self, pos: u64) {
        self.generation += 1;
        self.buf.clear();
        self.buf_start = pos;
        self.read_pos = pos;
        self.error = None;
        self.error_message = None;
    }

    fn fail(&mut self, kind: io::ErrorKind, message: String) {
        self.error = Some(kind);
        self.error_message = Some(message);
    }
}

impl HttpSource {
    /// Starts downloading `url` on `runtime` and blocks until the response headers arrive.
    ///
    /// `on_start` receives a [`SourceCloser`] before blocking, so another thread can abort the
    /// open (or any later blocking read). If the URL expires (HTTP 403/410), `refresher` is
    /// asked for a new one and the download resumes where it was.
    pub fn open(
        runtime: &tokio::runtime::Handle,
        client: reqwest::Client,
        url: Url,
        config: StreamConfig,
        refresher: Option<UrlRefresher>,
        on_start: impl FnOnce(SourceCloser),
    ) -> io::Result<Self> {
        assert!(
            config.back_bytes < config.buffer_bytes,
            "back_bytes must leave room for read-ahead"
        );
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                buf: VecDeque::new(),
                buf_start: 0,
                read_pos: 0,
                generation: 0,
                content_length: None,
                error: None,
                error_message: None,
                closed: false,
                peak_buffered: 0,
            }),
            data_ready: Condvar::new(),
            writer_wake: Notify::new(),
            config,
        });
        runtime.spawn(download(Arc::clone(&shared), client, url, refresher));
        on_start(SourceCloser(Arc::clone(&shared)));

        let mut state = shared.lock();
        let len = loop {
            if state.closed {
                return Err(closed_error());
            }
            if let Some(len) = state.content_length {
                break len;
            }
            if let Some(kind) = state.error {
                return Err(io::Error::new(
                    kind,
                    state.error_message.clone().unwrap_or_default(),
                ));
            }
            state = shared.wait(state);
        };
        drop(state);
        Ok(Self {
            shared,
            pos: 0,
            len,
        })
    }

    /// Total size of the file in bytes.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Largest number of bytes held in memory at once so far.
    #[must_use]
    pub fn peak_buffered(&self) -> usize {
        self.shared.lock().peak_buffered
    }
}

impl Read for HttpSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let config = self.shared.config;
        let mut state = self.shared.lock();
        loop {
            if state.closed {
                return Err(closed_error());
            }
            let pos = self.pos;
            if (state.buf_start..state.buf_end()).contains(&pos) {
                #[expect(clippy::cast_possible_truncation, reason = "bounded by buffer_bytes")]
                let offset = (pos - state.buf_start) as usize;
                let n = out.len().min(state.buf.len() - offset);
                let (a, b) = state.buf.as_slices();
                copy_from_ring(a, b, offset, &mut out[..n]);
                self.pos += n as u64;
                state.read_pos = self.pos;
                drop(state);
                self.shared.writer_wake.notify_one();
                return Ok(n);
            }

            let reachable =
                pos >= state.buf_start && pos <= state.buf_end() + config.forward_slack_bytes;
            if !reachable {
                tracing::debug!(pos, "seek outside window, reconnecting");
                state.restart_at(pos);
                self.shared.writer_wake.notify_one();
            } else if let Some(kind) = state.error {
                return Err(io::Error::new(
                    kind,
                    state.error_message.clone().unwrap_or_default(),
                ));
            }
            state = self.shared.wait(state);
        }
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        // The actual reconnect, if needed, happens lazily on the next read.
        self.pos = target.min(self.len);
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

impl Drop for HttpSource {
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// Closes an [`HttpSource`] from another thread: stops the download and makes blocked or future
/// reads fail.
#[derive(Debug, Clone)]
pub struct SourceCloser(Arc<Shared>);

impl SourceCloser {
    pub fn close(&self) {
        self.0.close();
    }
}

fn closed_error() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "stream closed")
}

impl Shared {
    fn close(&self) {
        self.lock().closed = true;
        self.data_ready.notify_all();
        self.writer_wake.notify_one();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock means the other side panicked mid-update; the state is still usable
        // for shutting down, which is all either side does after that.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.data_ready
            .wait(guard)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn copy_from_ring(a: &[u8], b: &[u8], offset: usize, out: &mut [u8]) {
    let n = out.len();
    if offset < a.len() {
        let from_a = n.min(a.len() - offset);
        out[..from_a].copy_from_slice(&a[offset..offset + from_a]);
        out[from_a..].copy_from_slice(&b[..n - from_a]);
    } else {
        let offset = offset - a.len();
        out.copy_from_slice(&b[offset..offset + n]);
    }
}

// ---- Download task -----------------------------------------------------------------------------

enum Outcome {
    /// Reader closed the source.
    Closed,
    /// Reader asked for a different position; reconnect there.
    Restart,
    /// Connection dropped or timed out; retry from where we were.
    Retry(String),
    /// The signed URL expired (HTTP 403/410).
    Expired(String),
    /// Unrecoverable (e.g. HTTP 404).
    Fatal(io::ErrorKind, String),
    /// Whole file downloaded.
    Finished,
}

async fn download(
    shared: Arc<Shared>,
    client: reqwest::Client,
    mut url: Url,
    refresher: Option<UrlRefresher>,
) {
    let mut retries = 0;
    // Refresh at most once per expiry, so a URL the refresher can't fix fails fast.
    let mut already_refreshed = false;
    loop {
        let (start, generation) = {
            let s = shared.lock();
            if s.closed {
                return;
            }
            (s.buf_end(), s.generation)
        };

        let outcome = fetch_from(&shared, &client, &mut url, start, generation).await;
        if !matches!(outcome, Outcome::Expired(_)) {
            already_refreshed = false;
        }
        match outcome {
            Outcome::Closed => return,
            Outcome::Restart => retries = 0,
            Outcome::Retry(message) if retries < MAX_RETRIES => {
                retries += 1;
                tracing::warn!(%message, retries, "stream interrupted, retrying");
                tokio::time::sleep(RETRY_BASE_DELAY * retries).await;
            }
            Outcome::Retry(message) => {
                fail(
                    &shared,
                    generation,
                    io::ErrorKind::ConnectionAborted,
                    message,
                );
                if wait_for_restart(&shared, generation).await {
                    retries = 0;
                } else {
                    return;
                }
            }
            Outcome::Expired(message) => {
                if !already_refreshed && let Some(refresh) = &refresher {
                    already_refreshed = true;
                    if let Some(fresh) = refresh(url.clone()).await {
                        tracing::info!(
                            "stream URL expired; continuing with a already_refreshed one"
                        );
                        url = fresh;
                        continue;
                    }
                }
                fail(
                    &shared,
                    generation,
                    io::ErrorKind::PermissionDenied,
                    message,
                );
                if wait_for_restart(&shared, generation).await {
                    retries = 0;
                } else {
                    return;
                }
            }
            Outcome::Fatal(kind, message) => {
                fail(&shared, generation, kind, message);
                if wait_for_restart(&shared, generation).await {
                    retries = 0;
                } else {
                    return;
                }
            }
            Outcome::Finished => {
                // Stay alive: a backward seek out of the window needs a new connection.
                if !wait_for_restart(&shared, generation).await {
                    return;
                }
            }
        }
    }
}

/// Downloads from byte `start` until the file ends, the reader seeks away, or an error.
async fn fetch_from(
    shared: &Shared,
    client: &reqwest::Client,
    url: &mut Url,
    start: u64,
    generation: u64,
) -> Outcome {
    let request = client
        .get(url.clone())
        .header(RANGE, format!("bytes={start}-"))
        .send();
    let response = tokio::select! {
        r = tokio::time::timeout(CONNECT_TIMEOUT, request) => match r {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => return Outcome::Retry(e.to_string()),
            Err(_) => return Outcome::Retry("timed out waiting for response".into()),
        },
        outcome = interrupted(shared, generation) => return outcome,
    };

    let status = response.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        return Outcome::Finished;
    }
    let total = match (status, total_length(&response)) {
        (StatusCode::PARTIAL_CONTENT, Some(total)) => total,
        // A server that ignores Range sends the whole file; only usable from the start.
        (StatusCode::OK, Some(total)) if start == 0 => total,
        (StatusCode::OK, _) => {
            return Outcome::Fatal(io::ErrorKind::Unsupported, "server ignored Range".into());
        }
        (s, _) if s.is_server_error() => return Outcome::Retry(format!("HTTP {s}")),
        // Signed CDN URLs expire after about a day.
        (StatusCode::FORBIDDEN | StatusCode::GONE, _) => {
            return Outcome::Expired(format!("HTTP {status}"));
        }
        (s, _) if !s.is_success() => {
            return Outcome::Fatal(io::ErrorKind::NotFound, format!("HTTP {s}"));
        }
        _ => return Outcome::Fatal(io::ErrorKind::InvalidData, "missing content length".into()),
    };
    // Reuse the resolved CDN URL so later range requests skip the redirect.
    url.clone_from(response.url());
    {
        let mut s = shared.lock();
        if s.generation != generation {
            return Outcome::Restart;
        }
        s.content_length = Some(total);
    }
    shared.data_ready.notify_all();

    let mut response = response;
    loop {
        let chunk = tokio::select! {
            c = response.chunk() => c,
            outcome = interrupted(shared, generation) => return outcome,
        };
        match chunk {
            Ok(Some(chunk)) => {
                if let Some(outcome) = store(shared, generation, chunk).await {
                    return outcome;
                }
            }
            Ok(None) => {
                let s = shared.lock();
                return if s.buf_end() >= total {
                    Outcome::Finished
                } else {
                    Outcome::Retry("connection closed early".into())
                };
            }
            Err(e) => return Outcome::Retry(e.to_string()),
        }
    }
}

/// Appends `chunk` to the window, waiting for space as needed. Returns `Some` if the download
/// should stop.
async fn store(shared: &Shared, generation: u64, mut chunk: Bytes) -> Option<Outcome> {
    let config = shared.config;
    while !chunk.is_empty() {
        {
            let mut s = shared.lock();
            if s.closed {
                return Some(Outcome::Closed);
            }
            if s.generation != generation {
                return Some(Outcome::Restart);
            }
            // Drop data the reader is done with, keeping `back_bytes` behind it.
            let keep_from = s.read_pos.saturating_sub(config.back_bytes as u64);
            if keep_from > s.buf_start {
                #[expect(clippy::cast_possible_truncation, reason = "bounded by buffer_bytes")]
                let drop_n = ((keep_from - s.buf_start) as usize).min(s.buf.len());
                s.buf.drain(..drop_n);
                s.buf_start += drop_n as u64;
            }
            let space = config.buffer_bytes.saturating_sub(s.buf.len());
            if space > 0 {
                let part = chunk.split_to(space.min(chunk.len()));
                s.buf.extend(part.iter());
                s.peak_buffered = s.peak_buffered.max(s.buf.len());
                drop(s);
                shared.data_ready.notify_all();
                continue;
            }
        }
        // Window full: wait until the reader frees space (or seeks/closes).
        shared.writer_wake.notified().await;
    }
    None
}

/// Resolves when the reader closes or restarts; never resolves otherwise.
async fn interrupted(shared: &Shared, generation: u64) -> Outcome {
    loop {
        shared.writer_wake.notified().await;
        let s = shared.lock();
        if s.closed {
            return Outcome::Closed;
        }
        if s.generation != generation {
            return Outcome::Restart;
        }
        // Just a "space freed" wake-up; `store` handles those.
    }
}

/// Parks the task until the reader requests a new position. Returns `false` if closed.
async fn wait_for_restart(shared: &Shared, generation: u64) -> bool {
    matches!(interrupted(shared, generation).await, Outcome::Restart)
}

fn fail(shared: &Shared, generation: u64, kind: io::ErrorKind, message: String) {
    tracing::warn!(%message, "stream failed");
    {
        let mut s = shared.lock();
        if s.generation != generation {
            return;
        }
        s.fail(kind, message);
    }
    shared.data_ready.notify_all();
}

/// Total file size from `Content-Range: bytes a-b/total`, or `Content-Length` for a 200.
fn total_length(response: &reqwest::Response) -> Option<u64> {
    if let Some(range) = response.headers().get(CONTENT_RANGE) {
        return range.to_str().ok()?.rsplit_once('/')?.1.parse().ok();
    }
    response
        .headers()
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}
