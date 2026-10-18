//! Cover art, shown only on terminals with a real graphics protocol (kitty, iTerm2, sixel).
//! Everywhere else the album pane stays text-only: no blocky half-block fallback.
//!
//! When art is disabled nothing here runs: no images are downloaded, decoded or cached.

use std::fmt;
use std::sync::Arc;

use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, FontSize, Resize};

use crate::bandcamp::images::ImageSize;
use crate::config::AlbumArt;

/// Cover heights we use, in rows. Snapping to a few steps means resizing the terminal only
/// re-encodes the image when it crosses a step, not on every column.
const ROW_STEPS: [u16; 6] = [6, 8, 10, 12, 16, 20];
/// The narrow single-pane layout keeps the cover small so more of the tracklist shows.
const COMPACT_MAX_ROWS: u16 = 8;
/// The text beside the cover always gets at least this many columns.
pub const MIN_TEXT_COLS: u16 = 28;
/// At least this many tracklist rows stay visible under the cover.
const MIN_TRACK_ROWS: u16 = 6;
/// The cover takes at most this share of the pane's height.
const MAX_HEIGHT_PERCENT: u16 = 45;

/// Where a cover fits, in terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArtBox {
    pub cols: u16,
    pub rows: u16,
}

/// How big the cover can be in a pane of `width`×`height` cells: as large as possible while
/// the text beside it and the tracklist below keep their room, snapped to [`ROW_STEPS`].
/// `None` when even the smallest step doesn't fit (then the pane is text-only).
#[must_use]
pub fn layout(width: u16, height: u16, font: FontSize, compact: bool) -> Option<ArtBox> {
    let (cell_w, cell_h) = (u32::from(font.width.max(1)), u32::from(font.height.max(1)));
    // Columns needed for `rows` rows so the cover is square in pixels.
    let cols_for = |rows: u16| (u32::from(rows) * cell_h).div_ceil(cell_w);
    let max_cols = u32::from(width.saturating_sub(MIN_TEXT_COLS + 1));
    let max_rows = (height * MAX_HEIGHT_PERCENT / 100)
        .min(height.saturating_sub(MIN_TRACK_ROWS + 1))
        .min(if compact { COMPACT_MAX_ROWS } else { u16::MAX });
    ROW_STEPS
        .iter()
        .rev()
        .copied()
        .find(|&rows| rows <= max_rows && cols_for(rows) <= max_cols)
        .and_then(|rows| {
            let cols = u16::try_from(cols_for(rows)).ok()?;
            Some(ArtBox { cols, rows })
        })
}

/// The smallest Bandcamp image that covers `art_box` at this font size without upscaling.
#[must_use]
pub fn source_size(art_box: ArtBox, font: FontSize) -> ImageSize {
    let pixels = (u32::from(art_box.cols) * u32::from(font.width))
        .max(u32::from(art_box.rows) * u32::from(font.height));
    match pixels {
        0..=150 => ImageSize::Px150,
        151..=350 => ImageSize::Px350,
        _ => ImageSize::Px700,
    }
}

/// A cover encoded for the terminal's graphics protocol at one [`ArtBox`] size.
#[derive(Clone)]
pub struct Art {
    protocol: Arc<Protocol>,
    pub art_box: ArtBox,
}

impl Art {
    #[must_use]
    pub fn protocol(&self) -> &Protocol {
        &self.protocol
    }
}

impl PartialEq for Art {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.protocol, &other.protocol)
    }
}

impl fmt::Debug for Art {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Art")
    }
}

/// The outcome of [`detect`].
#[derive(Debug, Default)]
pub struct Detection {
    pub picker: Option<Picker>,
    /// Why art is off despite `album_art = "on"`, for the user.
    pub note: Option<&'static str>,
}

/// Decides whether to show art, asking the terminal if needed. Call after entering the
/// alternate screen and before reading input events: the query reads the terminal's reply
/// from stdin.
#[must_use]
pub fn detect(mode: AlbumArt) -> Detection {
    let in_tmux = std::env::var_os("TMUX").is_some_and(|v| !v.is_empty());
    if !should_query(mode, in_tmux) {
        tracing::info!(?mode, in_tmux, "album art disabled");
        return Detection::default();
    }
    // Inside tmux, both the images and the capability query reach the real terminal only
    // through passthrough. Without it the query is never answered (see below), and images
    // couldn't be shown anyway.
    if in_tmux && !tmux_passthrough_enabled() {
        tracing::info!("tmux allow-passthrough is off; album art disabled");
        return Detection {
            picker: None,
            note: Some("Cover art in tmux needs `set -g allow-passthrough on` in ~/.tmux.conf."),
        };
    }
    Detection {
        picker: query_picker(mode),
        note: None,
    }
}

fn query_picker(mode: AlbumArt) -> Option<Picker> {
    // ratatui-image 11 reads the reply on a helper thread; if the terminal never answers,
    // that thread stays blocked reading stdin and swallows every key typed afterwards. So
    // first check, through crossterm's own input reader (which the app keeps using), that the
    // terminal answers queries at all. (Fixed upstream in ratatui-image 12; drop this then.)
    let answers = crossterm::terminal::supports_keyboard_enhancement().is_ok();
    let mut picker = if answers {
        match Picker::from_query_stdio() {
            Ok(picker) => picker,
            Err(e) => {
                tracing::info!(error = %e, "terminal graphics query failed; album art disabled");
                return None;
            }
        }
    } else if mode == AlbumArt::On {
        // No answer, but `on` trusts the environment. This picker has ratatui-image's default
        // 10×20 cell size; its protocol is replaced below.
        tracing::info!("terminal doesn't answer queries; using environment hints");
        Picker::halfblocks()
    } else {
        tracing::info!("terminal doesn't answer queries; album art disabled");
        return None;
    };
    let detected = if answers {
        picker.protocol_type()
    } else {
        ProtocolType::Halfblocks
    };
    let chosen = choose_protocol(mode, detected, protocol_from_env());
    tracing::info!(?detected, ?chosen, "album art");
    let protocol = chosen?;
    picker.set_protocol_type(protocol);
    Some(picker)
}

/// Whether tmux forwards escape sequences to the outer terminal for this pane.
fn tmux_passthrough_enabled() -> bool {
    std::process::Command::new("tmux")
        .args(["show-options", "-Apv", "allow-passthrough"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| matches!(String::from_utf8_lossy(&out.stdout).trim(), "on" | "all"))
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

/// Decodes a JPEG cover and encodes it for the terminal to fill `art_box`.
/// CPU-bound: call from a blocking task.
pub fn encode(picker: &Picker, jpeg: &[u8], art_box: ArtBox) -> Result<Art, String> {
    let image = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .map_err(|e| format!("couldn't decode cover: {e}"))?;
    let protocol = picker
        .new_protocol(
            image,
            Size::new(art_box.cols, art_box.rows),
            // Scale (unlike Fit) also enlarges, so the cover always fills its box.
            Resize::Scale(Some(FilterType::Triangle)),
        )
        .map_err(|e| format!("couldn't encode cover: {e}"))?;
    Ok(Art {
        protocol: Arc::new(protocol),
        art_box,
    })
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

    pub(crate) const TEST_BOX: ArtBox = ArtBox { cols: 16, rows: 8 };

    #[test]
    fn encodes_a_jpeg_and_rejects_garbage() {
        let picker = Picker::halfblocks();
        let art = encode(&picker, &test_jpeg(), TEST_BOX).unwrap();
        assert_eq!(art.art_box, TEST_BOX);
        assert!(art.protocol().size().width <= TEST_BOX.cols);
        assert!(art.protocol().size().height <= TEST_BOX.rows);
        assert!(encode(&picker, b"not a jpeg", TEST_BOX).is_err());
    }

    /// A common 10×20 px cell: two rows of pixels per column of pixels.
    const FONT: FontSize = FontSize::new(10, 20);

    #[test]
    fn grows_with_the_pane_in_steps() {
        // Roomy pane: limited by the 20-row cap.
        assert_eq!(
            layout(120, 60, FONT, false),
            Some(ArtBox { cols: 40, rows: 20 })
        );
        // Height-limited: 45 % of 30 rows is 13, so the 12-row step.
        assert_eq!(
            layout(120, 30, FONT, false),
            Some(ArtBox { cols: 24, rows: 12 })
        );
        // Width-limited: 60 - 29 = 31 columns for the cover, so 12 rows (24 cols) fits, 16 doesn't.
        assert_eq!(
            layout(60, 60, FONT, false),
            Some(ArtBox { cols: 24, rows: 12 })
        );
        // Just above the smallest step.
        assert_eq!(
            layout(41, 14, FONT, false),
            Some(ArtBox { cols: 12, rows: 6 })
        );
    }

    #[test]
    fn hides_when_there_is_no_room() {
        assert_eq!(layout(40, 60, FONT, false), None, "text needs 28 columns");
        assert_eq!(layout(120, 12, FONT, false), None, "tracklist needs 6 rows");
        assert_eq!(layout(0, 0, FONT, false), None);
    }

    #[test]
    fn compact_caps_the_size() {
        assert_eq!(
            layout(120, 60, FONT, true),
            Some(ArtBox { cols: 16, rows: 8 })
        );
        assert_eq!(
            layout(120, 14, FONT, true),
            Some(ArtBox { cols: 12, rows: 6 })
        );
    }

    #[test]
    fn stays_square_for_other_fonts() {
        // Square cells (some sixel terminals report this): as many columns as rows.
        let square = FontSize::new(16, 16);
        assert_eq!(
            layout(120, 30, square, false),
            Some(ArtBox { cols: 12, rows: 12 })
        );
        // Odd sizes round the width up, never squashing the cover.
        let odd = FontSize::new(9, 19);
        assert_eq!(
            layout(120, 30, odd, false),
            Some(ArtBox { cols: 26, rows: 12 })
        );
    }

    #[test]
    fn picks_the_smallest_source_that_covers_the_box() {
        assert_eq!(
            source_size(ArtBox { cols: 12, rows: 6 }, FONT),
            ImageSize::Px150
        );
        assert_eq!(
            source_size(ArtBox { cols: 24, rows: 12 }, FONT),
            ImageSize::Px350
        );
        assert_eq!(
            source_size(ArtBox { cols: 40, rows: 20 }, FONT),
            ImageSize::Px700
        );
    }
}
