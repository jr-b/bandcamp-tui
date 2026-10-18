//! The audio-callback side of a track: a rodio [`Source`] that drains a lock-free PCM ring.
//!
//! This runs inside the audio callback, so it never blocks, locks or allocates. If the decoder
//! falls behind (slow network), it outputs whole frames of silence and flags `starving` instead
//! of glitching.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use rodio::{ChannelCount, Sample, SampleRate, Source};
use rtrb::Consumer;

/// State shared between a segment, its decoder thread and the engine.
#[derive(Debug)]
pub(crate) struct SegmentStats {
    /// Track position where this segment starts (non-zero after a seek).
    pub(crate) start: Duration,
    pub(crate) sample_rate: SampleRate,
    /// Frames handed to the audio device so far.
    pub(crate) frames_played: AtomicU64,
    /// Set by the decoder once every sample has been pushed into the ring.
    pub(crate) decode_done: AtomicBool,
    /// Set by the segment when it has played everything (natural end of track).
    pub(crate) finished: AtomicBool,
    /// Set by the engine to stop this segment immediately (skip, seek, stop).
    pub(crate) cancelled: AtomicBool,
    /// True while waiting for the ring to fill (start-up or underrun).
    pub(crate) starving: AtomicBool,
}

impl SegmentStats {
    pub(crate) fn new(start: Duration, sample_rate: SampleRate) -> Self {
        Self {
            start,
            sample_rate,
            frames_played: AtomicU64::new(0),
            decode_done: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            starving: AtomicBool::new(true),
        }
    }

    /// Current playback position within the track.
    pub(crate) fn position(&self) -> Duration {
        let frames = self.frames_played.load(Ordering::Relaxed);
        let rate = u64::from(self.sample_rate.get());
        self.start
            + Duration::from_secs(frames / rate)
            + Duration::from_nanos((frames % rate) * 1_000_000_000 / rate)
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub(crate) struct Segment {
    ring: Consumer<Sample>,
    stats: Arc<SegmentStats>,
    channels: ChannelCount,
    /// Samples to buffer before (re)starting output after starving.
    prime_samples: usize,
    /// Index of the next sample within the current frame.
    channel: u16,
    /// Whether the current frame is real audio or padding silence.
    frame_is_silence: bool,
    primed: bool,
    frames_played: u64,
}

impl Segment {
    pub(crate) fn new(
        ring: Consumer<Sample>,
        stats: Arc<SegmentStats>,
        channels: ChannelCount,
        prime_samples: usize,
    ) -> Self {
        Self {
            ring,
            stats,
            channels,
            prime_samples,
            channel: 0,
            frame_is_silence: false,
            primed: false,
            frames_played: 0,
        }
    }

    /// Decides at a frame boundary whether the next frame is audio, silence, or the end.
    fn start_frame(&mut self) -> Option<()> {
        if self.stats.cancelled.load(Ordering::Acquire) {
            return None;
        }
        let available = self.ring.slots();
        let channels = usize::from(self.channels.get());
        if !self.primed {
            let done = self.stats.decode_done.load(Ordering::Acquire);
            if available >= self.prime_samples || (done && available >= channels) {
                self.primed = true;
                self.stats.starving.store(false, Ordering::Relaxed);
            } else if done {
                // Everything decoded and played.
                self.stats.finished.store(true, Ordering::Release);
                return None;
            }
        } else if available < channels {
            if self.stats.decode_done.load(Ordering::Acquire) {
                self.stats.finished.store(true, Ordering::Release);
                return None;
            }
            // Underrun: wait for a healthy buffer again rather than stuttering.
            self.primed = false;
            self.stats.starving.store(true, Ordering::Relaxed);
        }
        self.frame_is_silence = !self.primed;
        if self.primed {
            self.frames_played += 1;
            self.stats
                .frames_played
                .store(self.frames_played, Ordering::Relaxed);
        }
        Some(())
    }
}

impl Iterator for Segment {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.channel == 0 {
            self.start_frame()?;
        }
        self.channel = (self.channel + 1) % self.channels.get();
        if self.frame_is_silence {
            Some(0.0)
        } else {
            // `start_frame` checked a whole frame is available.
            Some(self.ring.pop().unwrap_or(0.0))
        }
    }
}

impl Source for Segment {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.stats.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Used when a stream doesn't report its channel layout.
pub(crate) const STEREO: ChannelCount = NonZero::<u16>::new(2).unwrap();

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = NonZero::<u32>::new(10).unwrap();

    fn setup(prime: usize) -> (rtrb::Producer<Sample>, Segment, Arc<SegmentStats>) {
        let (tx, rx) = rtrb::RingBuffer::new(64);
        let stats = Arc::new(SegmentStats::new(Duration::ZERO, RATE));
        let seg = Segment::new(rx, Arc::clone(&stats), STEREO, prime);
        (tx, seg, stats)
    }

    #[test]
    fn outputs_silence_until_primed_then_audio() {
        let (mut tx, mut seg, stats) = setup(4);
        // Not primed: one whole stereo frame of silence.
        assert_eq!(seg.next(), Some(0.0));
        assert_eq!(seg.next(), Some(0.0));
        assert!(stats.starving.load(Ordering::Relaxed));

        for s in [1.0, 2.0, 3.0, 4.0] {
            tx.push(s).unwrap();
        }
        let out: Vec<_> = (0..4).map(|_| seg.next().unwrap()).collect();
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
        assert!(!stats.starving.load(Ordering::Relaxed));
        assert_eq!(stats.frames_played.load(Ordering::Relaxed), 2);
        assert_eq!(stats.position(), Duration::from_millis(200));
    }

    #[test]
    fn underrun_emits_whole_silent_frames_and_reprimes() {
        let (mut tx, mut seg, stats) = setup(2);
        for s in [1.0, 2.0, 3.0] {
            tx.push(s).unwrap();
        }
        assert_eq!(seg.next(), Some(1.0));
        assert_eq!(seg.next(), Some(2.0));
        // Only half a frame left: a full frame of silence, never a misaligned sample.
        assert_eq!(seg.next(), Some(0.0));
        assert_eq!(seg.next(), Some(0.0));
        assert!(stats.starving.load(Ordering::Relaxed));
        tx.push(4.0).unwrap();
        assert_eq!(seg.next(), Some(3.0));
        assert_eq!(seg.next(), Some(4.0));
    }

    #[test]
    fn ends_after_decode_done_and_ring_drained() {
        let (mut tx, mut seg, stats) = setup(100);
        tx.push(1.0).unwrap();
        tx.push(2.0).unwrap();
        stats.decode_done.store(true, Ordering::Release);
        // Below the prime threshold, but decoding is done, so play what's there.
        assert_eq!(seg.next(), Some(1.0));
        assert_eq!(seg.next(), Some(2.0));
        assert_eq!(seg.next(), None);
        assert!(stats.is_finished());
    }

    #[test]
    fn cancel_stops_immediately_without_finishing() {
        let (mut tx, mut seg, stats) = setup(2);
        tx.push(1.0).unwrap();
        tx.push(2.0).unwrap();
        stats.cancel();
        assert_eq!(seg.next(), None);
        assert!(!stats.is_finished());
    }
}
