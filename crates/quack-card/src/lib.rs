//! Draws Quack's ops cards (§21.6, §21.8) as PNG images.
//!
//! `quack-text` builds a [`Card`] (the words and numbers); [`png`] lays it
//! out as SVG and draws it with resvg at twice its logical size, using
//! only the fonts bundled in `fonts/` (Inter and `JetBrains Mono`, both
//! under the SIL Open Font License), so an image looks the same on every
//! host. Nothing here reads the system's fonts or files.

#![allow(
    clippy::many_single_char_names,
    clippy::too_many_arguments,
    reason = "drawing code: x, y, w, h, and r are the clearest names, and shapes take them all"
)]

mod draw;
mod svg;

use std::sync::{Arc, OnceLock};

use quack_text::card::Card;
use resvg::{tiny_skia, usvg};

/// How many device pixels per logical pixel: sharp on high-density
/// screens, still small enough to upload every few minutes.
const SCALE: f32 = 2.0;

/// The bundled fonts.
const FONTS: [&[u8]; 4] = [
    include_bytes!("../fonts/Inter-400.ttf"),
    include_bytes!("../fonts/Inter-600.ttf"),
    include_bytes!("../fonts/Inter-700.ttf"),
    include_bytes!("../fonts/JetBrainsMono-500.ttf"),
];

/// Why a card couldn't be drawn. Both mean a bug in this crate.
#[derive(Debug, thiserror::Error)]
pub enum CardError {
    /// The SVG Quack wrote didn't parse.
    #[error("the card's SVG didn't parse: {0}")]
    Svg(#[from] usvg::Error),
    /// The image would be empty or too large to allocate.
    #[error("the card's size is out of range")]
    Size,
    /// The PNG encoder failed.
    #[error("the card's PNG couldn't be encoded: {0}")]
    Png(String),
}

/// The card as SVG text (what [`png`] draws; tests and previews use it).
pub fn svg(card: &Card) -> String {
    draw::card(card)
}

/// The card as a PNG image.
pub fn png(card: &Card) -> Result<Vec<u8>, CardError> {
    let opt = usvg::Options {
        font_family: "Inter".to_owned(),
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg(card), &opt)?;
    let size = tree
        .size()
        .to_int_size()
        .scale_by(SCALE)
        .ok_or(CardError::Size)?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).ok_or(CardError::Size)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(SCALE, SCALE),
        &mut pixmap.as_mut(),
    );
    if let Some(logo) = logo() {
        let (x, y, size) = draw::LOGO;
        #[allow(clippy::cast_precision_loss)] // 256 pixels.
        let s = size * SCALE / logo.width() as f32;
        pixmap.draw_pixmap(
            0,
            0,
            logo.as_ref(),
            &tiny_skia::PixmapPaint {
                quality: tiny_skia::FilterQuality::Bicubic,
                ..tiny_skia::PixmapPaint::default()
            },
            tiny_skia::Transform::from_row(s, 0.0, 0.0, s, x * SCALE, y * SCALE),
            None,
        );
    }
    pixmap
        .encode_png()
        .map_err(|e| CardError::Png(e.to_string()))
}

/// Quack's logo (the dashboard's `logo.webp` as PNG), decoded once.
fn logo() -> Option<&'static tiny_skia::Pixmap> {
    static LOGO: OnceLock<Option<tiny_skia::Pixmap>> = OnceLock::new();
    LOGO.get_or_init(|| {
        tiny_skia::Pixmap::decode_png(include_bytes!("../assets/quack-logo.png")).ok()
    })
    .as_ref()
}

/// The font database, loaded once.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        for font in FONTS {
            db.load_font_data(font.to_vec());
        }
        Arc::new(db)
    })
    .clone()
}

#[cfg(test)]
mod tests;
