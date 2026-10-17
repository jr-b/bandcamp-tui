//! Cover art, shown only on terminals with a real graphics protocol (kitty, iTerm2, sixel).
//! Everywhere else the album pane stays text-only: no blocky half-block fallback.
//!
//! When art is disabled nothing here runs: no images are downloaded, decoded or cached.

use std::fmt;
use std::sync::Arc;

use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, Resize};

use crate::config::AlbumArt;

/// Size of the cover in terminal cells. With the usual 1:2 cell aspect this is square; the
/// 150 px source image (Bandcamp size code 7) fills it on most fonts without upscaling.
pub const ART_COLS: u16 = 16;
pub const ART_ROWS: u16 = 8;

/// A cover encoded once for the terminal's graphics protocol; the source image is dropped.
#[derive(Clone)]
pub struct Art(Arc<Protocol>);

impl Art {
    #[must_use]
    pub fn protocol(&self) -> &Protocol {
        &self.0
    }
}

impl PartialEq for Art {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for Art {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Art")
    }
}

/// Decides whether to show art, asking the terminal if needed. Call after entering the
/// alternate screen and before reading input events: the query reads the terminal's reply
/// from stdin.
#[must_use]
pub fn detect(mode: AlbumArt) -> Option<Picker> {
    let in_tmux = std::env::var_os("TMUX").is_some_and(|v| !v.is_empty());
    if !should_query(mode, in_tmux) {
        tracing::info!(?mode, in_tmux, "album art disabled");
        return None;
    }
    let mut picker = match Picker::from_query_stdio() {
        Ok(picker) => picker,
        Err(e) => {
            tracing::info!(error = %e, "terminal graphics query failed; album art disabled");
            return None;
        }
    };
    let detected = picker.protocol_type();
    let chosen = choose_protocol(mode, detected, protocol_from_env());
    tracing::info!(?detected, ?chosen, "album art");
    let protocol = chosen?;
    picker.set_protocol_type(protocol);
    Some(picker)
}

/// `auto` never tries inside tmux (passthrough is often off and detection unreliable).
fn should_query(mode: AlbumArt, in_tmux: bool) -> bool {
    match mode {
        AlbumArt::Off => false,
        AlbumArt::Auto => !in_tmux,
        AlbumArt::On => true,
    }
}

/// Only real graphics protocols count. ratatui-image reports `Halfblocks` when it found
/// nothing; with `on`, fall back to what the environment says the terminal is.
fn choose_protocol(
    mode: AlbumArt,
    detected: ProtocolType,
    from_env: Option<ProtocolType>,
) -> Option<ProtocolType> {
    match detected {
        ProtocolType::Kitty | ProtocolType::Iterm2 | ProtocolType::Sixel => Some(detected),
        ProtocolType::Halfblocks if mode == AlbumArt::On => from_env,
        ProtocolType::Halfblocks => None,
    }
}

fn protocol_from_env() -> Option<ProtocolType> {
    let var = |name| std::env::var(name).unwrap_or_default();
    let term = var("TERM");
    let program = var("TERM_PROGRAM");
    if term.contains("kitty") || program == "ghostty" || !var("KITTY_WINDOW_ID").is_empty() {
        Some(ProtocolType::Kitty)
    } else if matches!(program.as_str(), "iTerm.app" | "WezTerm") {
        Some(ProtocolType::Iterm2)
    } else {
        None
    }
}

/// Decodes a JPEG cover and encodes it for the terminal at [`ART_COLS`]×[`ART_ROWS`] cells.
/// CPU-bound: call from a blocking task.
pub fn encode(picker: &Picker, jpeg: &[u8]) -> Result<Art, String> {
    let image = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .map_err(|e| format!("couldn't decode cover: {e}"))?;
    let protocol = picker
        .new_protocol(
            image,
            Size::new(ART_COLS, ART_ROWS),
            Resize::Fit(Some(FilterType::Triangle)),
        )
        .map_err(|e| format!("couldn't encode cover: {e}"))?;
    Ok(Art(Arc::new(protocol)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn tmux_needs_explicit_on() {
        assert!(!should_query(AlbumArt::Auto, true));
        assert!(should_query(AlbumArt::Auto, false));
        assert!(should_query(AlbumArt::On, true));
        assert!(!should_query(AlbumArt::Off, false));
    }

    #[test]
    fn never_falls_back_to_halfblocks() {
        use ProtocolType::{Halfblocks, Iterm2, Kitty, Sixel};
        assert_eq!(choose_protocol(AlbumArt::Auto, Kitty, None), Some(Kitty));
        assert_eq!(choose_protocol(AlbumArt::Auto, Sixel, None), Some(Sixel));
        assert_eq!(
            choose_protocol(AlbumArt::Auto, Halfblocks, Some(Kitty)),
            None
        );
        assert_eq!(
            choose_protocol(AlbumArt::On, Halfblocks, Some(Iterm2)),
            Some(Iterm2)
        );
        assert_eq!(choose_protocol(AlbumArt::On, Halfblocks, None), None);
    }

    /// A small JPEG made in memory.
    pub(crate) fn test_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_fn(150, 150, |x, y| {
            image::Rgb([
                u8::try_from(x).unwrap_or(0),
                u8::try_from(y).unwrap_or(0),
                128,
            ])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    #[test]
    fn encodes_a_jpeg_and_rejects_garbage() {
        let picker = Picker::halfblocks();
        assert!(encode(&picker, &test_jpeg()).is_ok());
        assert!(encode(&picker, b"not a jpeg").is_err());
    }
}
