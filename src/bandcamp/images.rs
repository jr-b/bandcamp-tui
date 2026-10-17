//! Image URLs on Bandcamp's CDN.
//!
//! Album art lives at `https://f4.bcbits.com/img/a{art_id}_{size}.jpg`; other images (band
//! photos, merch) at `https://f4.bcbits.com/img/{image_id}_{size}.jpg`.

use url::Url;

use super::types::{ArtId, ImageId};

const IMAGE_HOST: &str = "https://f4.bcbits.com/img/";

/// The square sizes we use, named by their Bandcamp size code. Pixel sizes were measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageSize {
    /// 100×100 (code `3`).
    Px100,
    /// 150×150 (code `7`). Enough for terminal rendering.
    Px150,
    /// 350×350 (code `2`).
    Px350,
    /// 700×700 (code `5`).
    Px700,
    /// 1200×1200 (code `10`).
    Px1200,
}

impl ImageSize {
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Px100 => 3,
            Self::Px150 => 7,
            Self::Px350 => 2,
            Self::Px700 => 5,
            Self::Px1200 => 10,
        }
    }
}

/// URL of an album cover.
#[must_use]
pub fn art_url(id: ArtId, size: ImageSize) -> Url {
    build(&format!("a{id}_{}.jpg", size.code()))
}

/// URL of a non-album image, e.g. a band photo.
#[must_use]
pub fn image_url(id: ImageId, size: ImageSize) -> Url {
    build(&format!("{id}_{}.jpg", size.code()))
}

fn build(file: &str) -> Url {
    // Both parts are fixed ASCII or digits, so this can't fail.
    #[allow(clippy::expect_used)]
    Url::parse(IMAGE_HOST)
        .and_then(|base| base.join(file))
        .expect("image URL is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_art_and_image_urls() {
        assert_eq!(
            art_url(ArtId(4_072_557_709), ImageSize::Px150).as_str(),
            "https://f4.bcbits.com/img/a4072557709_7.jpg"
        );
        assert_eq!(
            image_url(ImageId(13_989_985), ImageSize::Px350).as_str(),
            "https://f4.bcbits.com/img/13989985_2.jpg"
        );
    }
}
