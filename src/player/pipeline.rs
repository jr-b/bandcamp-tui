//! One track's decode pipeline: `HttpSource` → symphonia → PCM ring → [`Segment`] in the
//! rodio player queue. Runs on its own thread, which is the only place that blocks on the network.

use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use rodio::{Player, Sample};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};
use url::Url;

use super::engine::Msg;
use super::http_source::{HttpSource, SourceCloser, StreamConfig, UrlRefresher};
use super::segment::{STEREO, Segment, SegmentStats};

/// Everything a pipeline needs from the engine.
pub(crate) struct PipelineContext {
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) http: reqwest::Client,
    pub(crate) player: Arc<Player>,
    pub(crate) stream: StreamConfig,
    /// Renews expired CDN URLs mid-stream.
    pub(crate) refresher: Option<UrlRefresher>,
    /// Decoded audio buffered between decoder and audio device.
    pub(crate) pcm_buffer: Duration,
    pub(crate) engine: mpsc::Sender<Msg>,
}

impl std::fmt::Debug for PipelineContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineContext")
            .field("stream", &self.stream)
            .field("pcm_buffer", &self.pcm_buffer)
            .finish_non_exhaustive()
    }
}

/// Engine-side handle. Dropping it stops the pipeline and silences its audio immediately.
#[derive(Debug)]
pub(crate) struct Pipeline {
    pub(crate) id: u64,
    commands: mpsc::Sender<Command>,
    shared: Arc<Shared>,
}

#[derive(Debug)]
enum Command {
    Seek(Duration),
}

#[derive(Debug, Default)]
struct Shared {
    /// The segment currently queued for playback. Replaced on seek.
    segment: Mutex<Option<Arc<SegmentStats>>>,
    closer: Mutex<Option<SourceCloser>>,
    stopped: AtomicBool,
}

impl Pipeline {
    pub(crate) fn spawn(ctx: &Arc<PipelineContext>, id: u64, url: Url, start: Duration) -> Self {
        let (commands, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let worker = Worker {
            ctx: Arc::clone(ctx),
            shared: Arc::clone(&shared),
            commands: rx,
        };
        let spawned = thread::Builder::new()
            .name(format!("decode-{id}"))
            .spawn(move || {
                let engine = worker.ctx.engine.clone();
                let shared = Arc::clone(&worker.shared);
                if let Err(Failure { message, expired }) = worker.run(&url, start) {
                    // Errors after a stop are just the stream being torn down.
                    if !shared.stopped.load(Ordering::Acquire) {
                        let _ = engine.send(Msg::Failed {
                            id,
                            message,
                            expired,
                        });
                    }
                }
            });
        if let Err(e) = spawned {
            let _ = ctx.engine.send(Msg::Failed {
                id,
                message: format!("could not start decoder thread: {e}"),
                expired: false,
            });
        }
        Self {
            id,
            commands,
            shared,
        }
    }

    /// Stats of the segment currently queued, once the stream has opened.
    pub(crate) fn segment(&self) -> Option<Arc<SegmentStats>> {
        lock(&self.shared.segment).clone()
    }

    pub(crate) fn seek(&self, to: Duration) {
        // Silence the old position right away; the worker queues a new segment.
        if let Some(segment) = self.segment() {
            segment.cancel();
        }
        let _ = self.commands.send(Command::Seek(to));
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        if let Some(segment) = self.segment() {
            segment.cancel();
        }
        if let Some(closer) = lock(&self.shared.closer).take() {
            closer.close();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- Worker thread -----------------------------------------------------------------------------

/// Why a pipeline stopped.
struct Failure {
    message: String,
    /// The stream URL is no longer valid (expired signature); new track URLs are needed.
    expired: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            expired: false,
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// Whether a failure means the signed URL expired. Expired CDN URLs answer 403/410
/// (`PermissionDenied`). Expired `stream_redirect` URLs answer 404, like genuinely missing
/// files, so for those we check the URL's own expiry time (`ts`, unix seconds).
fn is_expiry(kind: std::io::ErrorKind, url: &Url) -> bool {
    match kind {
        std::io::ErrorKind::PermissionDenied => true,
        std::io::ErrorKind::NotFound => signature_expired(url),
        _ => false,
    }
}

fn signature_expired(url: &Url) -> bool {
    let Some(ts) = url
        .query_pairs()
        .find(|(k, _)| k == "ts")
        .and_then(|(_, v)| v.parse::<u64>().ok())
    else {
        return false;
    };
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .is_ok_and(|now| now.as_secs() >= ts)
}

struct Worker {
    ctx: Arc<PipelineContext>,
    shared: Arc<Shared>,
    commands: mpsc::Receiver<Command>,
}

struct Decoding {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    sample_rate: rodio::SampleRate,
    channels: rodio::ChannelCount,
}

impl Worker {
    fn run(self, url: &Url, start: Duration) -> Result<(), Failure> {
        let shared = Arc::clone(&self.shared);
        let source = HttpSource::open(
            &self.ctx.runtime,
            self.ctx.http.clone(),
            url.clone(),
            self.ctx.stream,
            self.ctx.refresher.clone(),
            |closer| {
                *lock(&shared.closer) = Some(closer.clone());
                // Dropped before we stored the closer: close it ourselves.
                if shared.stopped.load(Ordering::Acquire) {
                    closer.close();
                }
            },
        );
        if self.stopped() {
            return Ok(());
        }
        let source = source.map_err(|e| Failure {
            message: format!("could not open stream: {e}"),
            expired: is_expiry(e.kind(), url),
        })?;
        let mut d = open_decoder(source)?;

        let (mut producer, mut stats) = self.queue_segment(&mut d, start)?;
        // Reused across packets; reallocated only if a packet is larger than any before.
        let mut samples: Option<(SampleBuffer<Sample>, usize)> = None;
        loop {
            if self.stopped() {
                return Ok(());
            }
            while let Ok(Command::Seek(to)) = self.commands.try_recv() {
                (producer, stats) = self.queue_segment(&mut d, to)?;
            }

            let packet = match d.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    // End of track. Stay around so a seek backwards still works.
                    stats.decode_done.store(true, Ordering::Release);
                    match self.commands.recv() {
                        Ok(Command::Seek(to)) => {
                            (producer, stats) = self.queue_segment(&mut d, to)?;
                            continue;
                        }
                        Err(_) => return Ok(()),
                    }
                }
                Err(e) if self.stopped() => {
                    tracing::debug!(error = %e, "stream closed");
                    return Ok(());
                }
                Err(e) => {
                    let expired =
                        matches!(&e, SymphoniaError::IoError(io) if is_expiry(io.kind(), url));
                    return Err(Failure {
                        message: format!("stream error: {e}"),
                        expired,
                    });
                }
            };
            if packet.track_id() != d.track_id {
                continue;
            }
            let decoded = match d.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(e)) => {
                    // A corrupt frame: skip it, like every player does.
                    tracing::debug!(error = e, "skipping undecodable frame");
                    continue;
                }
                Err(e) => return Err(format!("decode error: {e}").into()),
            };
            let frames = decoded.capacity();
            let buf = match &mut samples {
                Some((buf, cap)) if *cap >= frames => buf,
                _ => {
                    let buf = SampleBuffer::new(frames as u64, *decoded.spec());
                    &mut samples.insert((buf, frames)).0
                }
            };
            buf.copy_interleaved_ref(decoded);

            match self.push(&mut producer, buf.samples(), &mut d)? {
                Some(next) => (producer, stats) = next,
                None if self.stopped() => return Ok(()),
                None => {}
            }
        }
    }

    fn stopped(&self) -> bool {
        self.shared.stopped.load(Ordering::Acquire)
    }

    /// Writes decoded samples into the ring, waiting while it's full. Handles seeks that arrive
    /// while waiting; returns the new segment if one was queued.
    #[expect(clippy::type_complexity, reason = "a one-off return type")]
    fn push(
        &self,
        producer: &mut rtrb::Producer<Sample>,
        mut samples: &[Sample],
        d: &mut Decoding,
    ) -> Result<Option<(rtrb::Producer<Sample>, Arc<SegmentStats>)>, String> {
        // Sleep for a quarter of the buffer while full: responsive, but only a few wake-ups a
        // second when paused.
        let wait = self.ctx.pcm_buffer / 4;
        while !samples.is_empty() {
            let (_, rest) = producer.push_partial_slice(samples);
            samples = rest;
            if samples.is_empty() {
                break;
            }
            match self.commands.recv_timeout(wait) {
                // The rest of this packet belongs to the old position; drop it.
                Ok(Command::Seek(to)) => return self.queue_segment(d, to).map(Some),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
            if self.stopped() {
                return Ok(None);
            }
        }
        Ok(None)
    }

    /// Seeks (if `start` > 0) and queues a fresh segment for playback from there.
    fn queue_segment(
        &self,
        d: &mut Decoding,
        start: Duration,
    ) -> Result<(rtrb::Producer<Sample>, Arc<SegmentStats>), String> {
        if let Some(old) = lock(&self.shared.segment).as_ref() {
            old.cancel();
        }
        let (position, past_end) = if start.is_zero() {
            (Duration::ZERO, false)
        } else {
            match seek(d, start) {
                Ok(landed) => (landed, false),
                // Seeking past the last frame: queue an empty segment that ends at once.
                Err(SymphoniaError::SeekError(_)) => (start, true),
                Err(e) => return Err(format!("seek failed: {e}")),
            }
        };

        let channels = usize::from(d.channels.get());
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a short buffer duration times a sample rate"
        )]
        let ring_frames =
            (self.ctx.pcm_buffer.as_secs_f64() * f64::from(d.sample_rate.get())).ceil() as usize;
        let ring_samples = ring_frames.max(1024) * channels;
        let (producer, consumer) = rtrb::RingBuffer::new(ring_samples);
        let stats = Arc::new(SegmentStats::new(position, d.sample_rate));
        stats.decode_done.store(past_end, Ordering::Release);
        let segment = Segment::new(consumer, Arc::clone(&stats), d.channels, ring_samples / 2);

        let mut current = lock(&self.shared.segment);
        if self.stopped() {
            return Err("stopped".into());
        }
        self.ctx.player.append(segment);
        *current = Some(Arc::clone(&stats));
        drop(current);
        let _ = self.ctx.engine.send(Msg::Ready);
        Ok((producer, stats))
    }
}

fn open_decoder(source: HttpSource) -> Result<Decoding, String> {
    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let format_opts = FormatOptions {
        enable_gapless: true,
        ..FormatOptions::default()
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &MetadataOptions::default())
        .map_err(|e| format!("unsupported stream: {e}"))?;
    let format = probed.format;
    let track = format.default_track().ok_or("stream has no audio track")?;
    let params = track.codec_params.clone();
    let decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| format!("unsupported codec: {e}"))?;
    let sample_rate = params
        .sample_rate
        .and_then(NonZero::new)
        .ok_or("stream has no sample rate")?;
    let channels = params
        .channels
        .and_then(|c| u16::try_from(c.count()).ok())
        .and_then(NonZero::new)
        .unwrap_or(STEREO);
    Ok(Decoding {
        track_id: track.id,
        time_base: params.time_base,
        format,
        decoder,
        sample_rate,
        channels,
    })
}

/// Seeks to `to` and returns where we actually landed.
fn seek(d: &mut Decoding, to: Duration) -> Result<Duration, SymphoniaError> {
    // Coarse: jump to the estimated byte offset (exact for CBR) instead of parsing every frame
    // from the start, which would download everything in between.
    let seeked = d.format.seek(
        SeekMode::Coarse,
        SeekTo::Time {
            time: Time::from(to.as_secs_f64()),
            track_id: Some(d.track_id),
        },
    )?;
    d.decoder.reset();
    let landed = d.time_base.map_or(to, |tb| {
        let t = tb.calc_time(seeked.actual_ts);
        Duration::from_secs(t.seconds) + Duration::from_secs_f64(t.frac)
    });
    Ok(landed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_expiry() {
        let past: Url =
            "https://bandcamp.com/stream_redirect?enc=mp3-128&track_id=1&ts=1700000000&t=x"
                .parse()
                .unwrap();
        let future: Url = "https://bandcamp.com/stream_redirect?ts=99999999999"
            .parse()
            .unwrap();
        let unsigned: Url = "https://example.com/gone.mp3".parse().unwrap();
        assert!(is_expiry(std::io::ErrorKind::NotFound, &past));
        assert!(!is_expiry(std::io::ErrorKind::NotFound, &future));
        assert!(!is_expiry(std::io::ErrorKind::NotFound, &unsigned));
        assert!(is_expiry(std::io::ErrorKind::PermissionDenied, &unsigned));
        assert!(!is_expiry(std::io::ErrorKind::ConnectionAborted, &past));
    }
}
